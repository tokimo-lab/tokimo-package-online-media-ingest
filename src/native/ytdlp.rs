use std::fmt::Write as _;
use std::path::Path;
use std::process::Stdio;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::{Duration, sleep};
use tracing::{info, warn};

use crate::AppState;
use crate::engine::format::sanitize_filename;
use crate::engine::postprocess::{
    PostProcessOptions, parse_chapters_from_metadata, postprocess, write_chapters_file,
};
use crate::models::CreateTaskRequest;
use crate::music::{
    build_audio_metadata_pairs, detect_ytdlp_content_type, extract_music_metadata,
    is_music_content_type, music_metadata_from_task,
};
use crate::tooling::resolve_ytdlp_binary;

/// Result of analyzing a URL via yt-dlp.
#[derive(Debug, Clone)]
pub struct AnalyzeResult {
    pub normalized_url: String,
    pub source_id: Option<String>,
    pub external_id: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub thumbnail_url: Option<String>,
    pub duration_seconds: Option<u64>,
    pub uploader: Option<String>,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub track_title: Option<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub genre: Option<String>,
    pub release_date: Option<String>,
    pub content_type: Option<String>,
    pub requires_auth: Option<bool>,
    pub warnings: Vec<String>,
    pub raw_metadata: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DownloadProgress {
    downloaded_bytes: Option<u64>,
    total_bytes: Option<u64>,
    speed_bytes: Option<u64>,
    eta_seconds: Option<u64>,
}

fn should_download_audio_only(request: &CreateTaskRequest) -> bool {
    request.audio_only.unwrap_or_else(|| {
        is_music_content_type(&request.target_folder_config_snapshot.content_type)
    })
}

/// Analyze a URL using yt-dlp `--dump-json`.
pub async fn ytdlp_analyze(url: &str) -> Result<AnalyzeResult, String> {
    let ytdlp = resolve_ytdlp_binary().ok_or_else(|| "yt-dlp binary not found".to_string())?;

    let output = Command::new(&ytdlp)
        .args([
            "--dump-json",
            "--no-playlist",
            "--no-download",
            "--no-warnings",
            "--skip-download",
            url,
        ])
        .output()
        .await
        .map_err(|err| format!("failed to run yt-dlp: {err}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("yt-dlp analyze failed: {stderr}"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let info: Value = serde_json::from_str(stdout.trim())
        .map_err(|err| format!("failed to parse yt-dlp JSON: {err}"))?;

    let title = json_str(&info, "title").map(String::from);
    let description = json_str(&info, "description").map(String::from);
    let source_id = json_str(&info, "id").map(String::from);
    let thumbnail = json_str(&info, "thumbnail").map(String::from);
    let uploader = json_str(&info, "uploader")
        .or_else(|| json_str(&info, "channel"))
        .map(String::from);
    let duration = info
        .get("duration")
        .and_then(serde_json::Value::as_f64)
        .map(|d| d as u64);
    let webpage_url = json_str(&info, "webpage_url").unwrap_or(url).to_string();
    let music_metadata = extract_music_metadata(Some(&info));

    Ok(AnalyzeResult {
        normalized_url: webpage_url,
        source_id,
        external_id: json_str(&info, "display_id").map(String::from),
        title,
        description,
        thumbnail_url: thumbnail,
        duration_seconds: duration,
        uploader,
        artist: music_metadata.artist,
        album_artist: music_metadata.album_artist,
        album: music_metadata.album,
        track_title: music_metadata.title,
        track_number: music_metadata.track_number,
        disc_number: music_metadata.disc_number,
        genre: music_metadata.genre,
        release_date: music_metadata.release_date,
        content_type: Some(detect_ytdlp_content_type(Some(&info))),
        requires_auth: None,
        warnings: Vec::new(),
        raw_metadata: Some(info),
    })
}

/// Download a URL using yt-dlp. Files are written into `staging_dir`.
#[allow(clippy::too_many_lines)]
pub async fn ytdlp_download(
    state: &AppState,
    task_id: &str,
    request: &CreateTaskRequest,
    staging_dir: &Path,
) -> Result<(), String> {
    let ytdlp = resolve_ytdlp_binary().ok_or_else(|| "yt-dlp binary not found".to_string())?;

    let audio_only = should_download_audio_only(request);

    let title = request
        .metadata
        .title
        .as_deref()
        .or(request.metadata.media_title.as_deref())
        .unwrap_or("%(title)s");

    let output_template = staging_dir.join(format!("{}.%(ext)s", sanitize_filename(title)));

    let mut args: Vec<String> = vec![
        "--no-playlist".into(),
        "--no-warnings".into(),
        "--newline".into(),
        "--progress-template".into(),
        "download:[nex]%(progress.downloaded_bytes)s|%(progress.total_bytes)s|%(progress.total_bytes_estimate)s|%(progress.speed)s|%(progress.eta)s".into(),
        "--write-info-json".into(),
        "-o".into(),
        output_template.to_string_lossy().into_owned(),
    ];

    // Pass cookies from auth
    if let Some(cookie_header) = request
        .auth
        .as_ref()
        .and_then(|a| a.cookie_header.as_deref())
    {
        let cookie_file = staging_dir.join("_cookies.txt");
        write_cookie_file(&cookie_file, &request.url, cookie_header).await?;
        args.extend([
            "--cookies".into(),
            cookie_file.to_string_lossy().into_owned(),
        ]);
    }

    if audio_only {
        // Audio-only: just download the best audio stream.  Format conversion
        // (e.g. opus → mp3) is handled after download via our FFI transcoder,
        // so yt-dlp does not need ffmpeg at all.
        args.extend(["-f".into(), "bestaudio/best".into()]);
    } else {
        // Video: download best video + audio separately.  Do NOT pass
        // --ffmpeg-location or --merge-output-format — merging is handled
        // by our FFI-based merge_av after download completes.
        args.extend([
            "-f".into(),
            "bestvideo+bestaudio/best".into(),
            // Prefer MP4 output when yt-dlp merges via system ffmpeg.
            // If yt-dlp can't merge (no system ffmpeg), our FFI handles it.
            "--merge-output-format".into(),
            "mp4".into(),
        ]);
    }

    args.push(request.url.clone());

    info!(
        task_id = %task_id,
        url = %request.url,
        "starting yt-dlp download"
    );

    let mut child = Command::new(&ytdlp)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("failed to spawn yt-dlp: {err}"))?;

    // yt-dlp outputs progress and info messages to stdout (stderr is empty
    // when --no-warnings is used). Read stdout for [nex] progress lines.
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "failed to capture yt-dlp stdout".to_string())?;
    let mut stdout_lines = BufReader::new(stdout).lines();
    let mut output_buffer = Vec::new();

    loop {
        tokio::select! {
            line = stdout_lines.next_line() => {
                let Some(line) = line
                    .map_err(|err| format!("failed to read yt-dlp output: {err}"))?
                else {
                    break;
                };

        if let Some(progress) = parse_progress_line(&line) {
            state
                .tasks
                .update_download_progress(
                    task_id,
                    None,
                    progress.downloaded_bytes,
                    progress.total_bytes,
                    progress.speed_bytes,
                    progress.eta_seconds,
                )
                .await;
            continue;
        }

        let trimmed = line.trim();
        if !trimmed.is_empty() {
            output_buffer.push(trimmed.to_string());
        }
            }
            () = sleep(Duration::from_millis(250)) => {
                if state.tasks.is_cancel_requested(task_id).await {
                    let _ = child.kill().await;
                    return Err("task cancelled".into());
                }
            }
        }
    }

    let status = child
        .wait()
        .await
        .map_err(|err| format!("failed to wait for yt-dlp: {err}"))?;

    if !status.success() {
        // Collect stderr for error details, fall back to stdout buffer
        let mut error_msg = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            use tokio::io::AsyncReadExt;
            let mut buf = String::new();
            let _ = stderr.read_to_string(&mut buf).await;
            error_msg = buf.trim().to_string();
        }
        if error_msg.is_empty() {
            error_msg = output_buffer.join("\n");
        }
        return Err(format!("yt-dlp download failed: {error_msg}"));
    }

    // Clean up temporary files
    let _ = tokio::fs::remove_file(staging_dir.join("_cookies.txt")).await;
    cleanup_ytdlp_artifacts(staging_dir).await;

    // Merge separate video+audio streams via FFI.  Since we don't ask yt-dlp
    // to merge (no --merge-output-format / --ffmpeg-location for video), it
    // leaves format-coded files (e.g. title.f401.mp4 + title.f251.webm).
    if !audio_only && let Err(err) = try_merge_separate_streams(staging_dir).await {
        warn!(error = %err, "AV merge failed, continuing with separate files");
    }

    // Post-processing (all FFI-based, no external ffmpeg binary needed).
    if audio_only {
        // Audio format conversion via FFI (e.g. opus → mp3) if requested.
        if let Some(target_fmt) = request.audio_container.as_deref()
            && let Err(err) = try_convert_audio(staging_dir, target_fmt).await
        {
            warn!(error = %err, target_fmt, "audio format conversion failed, keeping original");
        }

        let task_music_metadata = music_metadata_from_task(&request.metadata);
        let raw_music_metadata = extract_music_metadata(request.metadata.raw_metadata.as_ref());
        let music_metadata = task_music_metadata.merge_with(&raw_music_metadata);
        let metadata_pairs = build_audio_metadata_pairs(
            &music_metadata,
            request
                .metadata
                .track_title
                .as_deref()
                .or(request.metadata.media_title.as_deref())
                .or(request.metadata.title.as_deref()),
            request
                .metadata
                .artist
                .as_deref()
                .or(request.metadata.uploader.as_deref()),
        );

        if !metadata_pairs.is_empty() {
            match find_media_file(staging_dir).await {
                Ok(media_file) => {
                    let audio_container = media_file
                        .extension()
                        .and_then(|value| value.to_str())
                        .map(str::to_ascii_lowercase);
                    let pp_options = PostProcessOptions {
                        metadata: metadata_pairs,
                        audio_only: true,
                        audio_container,
                        ..Default::default()
                    };
                    if let Err(err) = postprocess(&media_file, &pp_options).await {
                        warn!(error = %err, "audio metadata embedding failed, continuing without");
                    }
                }
                Err(err) => {
                    warn!(error = %err, "audio metadata embedding skipped");
                }
            }
        }
    } else {
        // Video post-processing: embed chapters (if any) and ensure MP4
        // faststart (moov atom at file start) for efficient Range-based
        // streaming over VFS backends (SMB, NFS, etc.).
        let chapters_file = if let Some(metadata) = request.metadata.raw_metadata.as_ref() {
            let chapters = parse_chapters_from_metadata(metadata);
            if chapters.is_empty() {
                None
            } else if let Ok(_media_file) = find_media_file(staging_dir).await {
                write_chapters_file(staging_dir, &chapters).await.ok()
            } else {
                None
            }
        } else {
            None
        };

        let is_mp4 = find_media_file(staging_dir).await.is_ok_and(|f| {
            f.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("mp4") || e.eq_ignore_ascii_case("m4v"))
        });

        let needs_pp = chapters_file.is_some() || is_mp4;
        if needs_pp && let Ok(media_file) = find_media_file(staging_dir).await {
            let pp_options = PostProcessOptions {
                chapters_file: chapters_file.clone(),
                faststart: is_mp4,
                ..Default::default()
            };
            if let Err(err) = postprocess(&media_file, &pp_options).await {
                warn!(error = %err, "video post-processing failed, continuing without");
            }
            if let Some(ref cf) = chapters_file {
                let _ = tokio::fs::remove_file(cf).await;
            }
        }
    }

    info!(task_id = %task_id, "yt-dlp download complete");
    Ok(())
}

fn json_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(|v| v.as_str())
}

fn parse_progress_line(line: &str) -> Option<DownloadProgress> {
    let payload = line.split("[nex]").nth(1)?;
    let mut parts = payload.split('|');

    let downloaded_bytes = parse_progress_number(parts.next()?);
    let total_bytes_raw = parse_progress_number(parts.next()?);
    let total_bytes_estimate = parse_progress_number(parts.next()?);
    let total_bytes = total_bytes_raw.or(total_bytes_estimate);
    let speed_bytes = parse_progress_number(parts.next()?);
    let eta_seconds = parse_progress_number(parts.next()?);

    Some(DownloadProgress {
        downloaded_bytes,
        total_bytes,
        speed_bytes,
        eta_seconds,
    })
}

fn parse_progress_number(value: &str) -> Option<u64> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("na") || trimmed == "None" {
        return None;
    }

    trimmed.parse::<f64>().ok().and_then(|number| {
        if number.is_finite() && number >= 0.0 {
            Some(number.round() as u64)
        } else {
            None
        }
    })
}

pub(crate) async fn write_cookie_file(
    path: &Path,
    url: &str,
    cookie_header: &str,
) -> Result<(), String> {
    let domain = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(String::from))
        .unwrap_or_else(|| ".example.com".into());

    let mut content = String::from("# Netscape HTTP Cookie File\n");
    for pair in cookie_header.split(';') {
        let pair = pair.trim();
        if let Some((name, value)) = pair.split_once('=') {
            let _ = writeln!(
                content,
                ".{domain}\tTRUE\t/\tFALSE\t0\t{}\t{}",
                name.trim(),
                value.trim()
            );
        }
    }

    tokio::fs::write(path, content.as_bytes())
        .await
        .map_err(|err| format!("failed to write cookie file: {err}"))
}

/// Regex pattern to detect yt-dlp format codes in filenames (e.g. `.f100026.mp4`).
static FORMAT_CODE_RE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"\.f\d+\.(\w+)$").unwrap());

/// If yt-dlp left separate video + audio files (failed merge), merge them via FFI.
///
/// Detects files with format codes (e.g. `title.f100026.mp4` + `title.f30280.m4a`)
/// and merges them into a single `title.mp4`.
///
/// Uses ffprobe FFI to determine which file contains a video stream and which is
/// audio-only. This correctly handles `YouTube`'s common pattern of `f401.mp4` (AV1
/// video) + `f251.webm` (Opus audio), where both extensions would otherwise look
/// like video containers.
async fn try_merge_separate_streams(staging_dir: &Path) -> Result<(), String> {
    let mut entries = tokio::fs::read_dir(staging_dir)
        .await
        .map_err(|err| format!("failed to read staging dir: {err}"))?;

    let media_exts = [
        "mp4", "mkv", "webm", "avi", "mov", "m4a", "mp3", "aac", "ogg", "opus", "flac", "wav",
    ];

    let mut format_coded_files: Vec<std::path::PathBuf> = Vec::new();

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !FORMAT_CODE_RE.is_match(name) {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        if !media_exts.contains(&ext.as_str()) {
            continue;
        }
        format_coded_files.push(path);
    }

    if format_coded_files.len() != 2 {
        if !format_coded_files.is_empty() {
            info!(
                count = format_coded_files.len(),
                "skipping AV merge: expected 2 format-coded files"
            );
        }
        return Ok(());
    }

    // Probe both files to identify which has a video stream.
    let mut video_file: Option<std::path::PathBuf> = None;
    let mut audio_file: Option<std::path::PathBuf> = None;

    for path in &format_coded_files {
        let path_clone = path.clone();
        let path_str = path.to_string_lossy().into_owned();
        let probe_result =
            tokio::task::spawn_blocking(move || tokimo_package_ffmpeg::probe_file(&path_str)).await;

        let has_video = match &probe_result {
            Ok(Ok(info)) => info.streams.iter().any(|s| s.video.is_some()),
            Ok(Err(e)) => {
                warn!(
                    path = %path_clone.display(),
                    error = %e,
                    "ffprobe failed during AV merge detection"
                );
                false
            }
            Err(e) => {
                warn!(
                    path = %path_clone.display(),
                    error = %e,
                    "ffprobe task panicked during AV merge detection"
                );
                false
            }
        };

        if has_video && video_file.is_none() {
            video_file = Some(path_clone);
        } else if !has_video && audio_file.is_none() {
            audio_file = Some(path_clone);
        } else if has_video && video_file.is_some() {
            info!(
                file_a = %video_file.as_deref().unwrap().display(),
                file_b = %path_clone.display(),
                "skipping AV merge: both files contain video streams"
            );
            return Ok(());
        }
    }

    let (Some(video), Some(audio)) = (video_file, audio_file) else {
        let files: Vec<_> = format_coded_files
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        info!(
            ?files,
            "skipping AV merge: could not identify separate video and audio files"
        );
        return Ok(());
    };

    // Derive merged output name by stripping the format code
    let video_name = video
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("merged.mp4");
    let merged_name = FORMAT_CODE_RE.replace(video_name, ".mp4");
    let merged_path = staging_dir.join(merged_name.as_ref());

    info!(
        video = %video.display(),
        audio = %audio.display(),
        output = %merged_path.display(),
        "merging separate video+audio streams via FFI"
    );

    let v = video.clone();
    let a = audio.clone();
    let out = merged_path.clone();
    tokio::task::spawn_blocking(move || {
        tokimo_package_ffmpeg::merge_av(&v, &a, &out)
            .map_err(|e| format!("FFI merge_av failed: {e}"))
    })
    .await
    .map_err(|e| format!("task join error: {e}"))??;

    // Remove the separate files
    let _ = tokio::fs::remove_file(&video).await;
    let _ = tokio::fs::remove_file(&audio).await;

    info!(output = %merged_path.display(), "AV merge complete");
    Ok(())
}

/// Convert the downloaded audio file to the requested format via FFI transcoding.
///
/// Probes the file to detect source sample rate / channels, then transcodes.
/// Skips conversion when the file already has the target extension.
async fn try_convert_audio(staging_dir: &Path, target_fmt: &str) -> Result<(), String> {
    let media_file = find_media_file(staging_dir).await?;

    let current_ext = media_file
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();

    // Normalise target so that e.g. "m4a" matches the "aac" encoder path.
    let normalised_target = target_fmt;

    if current_ext == normalised_target {
        return Ok(());
    }

    // Probe source to preserve sample rate and channel count.
    let probe_path = media_file.to_string_lossy().into_owned();
    let probe_info =
        tokio::task::spawn_blocking(move || tokimo_package_ffmpeg::probe_file(&probe_path))
            .await
            .map_err(|e| format!("probe task error: {e}"))?
            .map_err(|e| format!("failed to probe audio file: {e}"))?;

    let audio_stream = probe_info
        .streams
        .iter()
        .find(|s| s.audio.is_some())
        .ok_or("no audio stream found in downloaded file")?;
    let audio = audio_stream.audio.as_ref().unwrap();

    let sample_rate = audio.sample_rate.parse::<u32>().unwrap_or(44100);
    let channels = (audio.channels as u8).max(1);

    let output_path = media_file.with_extension(normalised_target);
    let input = media_file.clone();
    let output = output_path.clone();
    let fmt = normalised_target.to_string();

    info!(
        input = %media_file.display(),
        output = %output_path.display(),
        "converting audio format via FFI"
    );

    tokio::task::spawn_blocking(move || {
        tokimo_package_ffmpeg::audio::convert_audio_file(
            &input,
            &output,
            &tokimo_package_ffmpeg::audio::AudioConvertOptions {
                sample_rate,
                channels,
                output_format: fmt,
            },
        )
        .map_err(|e| format!("FFI audio conversion failed: {e}"))
    })
    .await
    .map_err(|e| format!("task join error: {e}"))??;

    // Remove original if output is a different file.
    if output_path != media_file {
        let _ = tokio::fs::remove_file(&media_file).await;
    }

    info!(output = %output_path.display(), "audio format conversion complete");
    Ok(())
}

async fn cleanup_ytdlp_artifacts(staging_dir: &Path) {
    let Ok(mut entries) = tokio::fs::read_dir(staging_dir).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if let Some(name) = path.file_name().and_then(|n| n.to_str())
            && (name.ends_with(".info.json")
                || std::path::Path::new(name)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("part"))
                || std::path::Path::new(name)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("ytdl"))
                || name.starts_with('_'))
        {
            let _ = tokio::fs::remove_file(&path).await;
        }
    }
}

async fn find_media_file(staging_dir: &Path) -> Result<std::path::PathBuf, String> {
    let mut entries = tokio::fs::read_dir(staging_dir)
        .await
        .map_err(|err| format!("failed to read staging dir: {err}"))?;

    let media_extensions = [
        "mp4", "mkv", "webm", "m4a", "mp3", "flac", "ogg", "avi", "mov",
    ];

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if let Some(ext) = path.extension().and_then(|e| e.to_str())
            && media_extensions.contains(&ext.to_ascii_lowercase().as_str())
        {
            return Ok(path);
        }
    }

    Err("no media file found in staging directory".into())
}

#[cfg(test)]
mod tests {
    use super::{DownloadProgress, parse_progress_line};

    #[test]
    fn parses_progress_with_total_bytes() {
        let progress = parse_progress_line("download:[nex]1048576|2097152|NA|524288|12");
        assert_eq!(
            progress,
            Some(DownloadProgress {
                downloaded_bytes: Some(1_048_576),
                total_bytes: Some(2_097_152),
                speed_bytes: Some(524_288),
                eta_seconds: Some(12),
            })
        );
    }

    #[test]
    fn parses_progress_with_estimated_total_bytes() {
        let progress = parse_progress_line("download:[nex]1048576|NA|3145728|262144|8");
        assert_eq!(
            progress,
            Some(DownloadProgress {
                downloaded_bytes: Some(1_048_576),
                total_bytes: Some(3_145_728),
                speed_bytes: Some(262_144),
                eta_seconds: Some(8),
            })
        );
    }
}
