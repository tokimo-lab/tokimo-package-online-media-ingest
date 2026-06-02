use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use tracing::{info, warn};

/// Post-processing options to apply after download.
#[derive(Debug, Clone, Default)]
pub struct PostProcessOptions {
    /// Subtitle file to embed into the output container.
    pub subtitle_file: Option<PathBuf>,
    /// Subtitle language code (e.g. "eng", "chi", "jpn").
    pub subtitle_language: Option<String>,
    /// Chapter metadata file (ffmetadata format) to embed.
    pub chapters_file: Option<PathBuf>,
    /// Cover/thumbnail image to embed as attached picture.
    pub cover_file: Option<PathBuf>,
    /// Metadata key-value pairs to embed (e.g. title, artist, comment).
    pub metadata: Vec<(String, String)>,
    /// If true, extract audio only (discard video track).
    pub audio_only: bool,
    /// Target audio container when `audio_only` is true. When omitted, keep the current extension.
    pub audio_container: Option<String>,
    /// If true, move moov atom to file start (MP4 faststart) even when no
    /// other post-processing is requested.  This is critical for MP4 files
    /// served over VFS (SMB/NFS) — without it, the browser's `<video>`
    /// element must seek between the file start and end to parse the moov
    /// atom, causing hundreds of Range requests.
    pub faststart: bool,
}

impl PostProcessOptions {
    /// Returns true if any post-processing is requested.
    pub fn has_work(&self) -> bool {
        self.subtitle_file.is_some()
            || self.chapters_file.is_some()
            || self.cover_file.is_some()
            || !self.metadata.is_empty()
            || self.audio_only
            || self.faststart
    }

    /// Returns true if the only operation is adding faststart (no other embedding).
    /// In this case we can use a subprocess ffmpeg invocation instead of FFI remux,
    /// which is more robust for exotic codec/container combinations (e.g. AV1 in MP4).
    fn is_faststart_only(&self) -> bool {
        self.faststart
            && self.subtitle_file.is_none()
            && self.chapters_file.is_none()
            && self.cover_file.is_none()
            && self.metadata.is_empty()
            && !self.audio_only
    }
}

/// Apply post-processing to a media file in-place using `FFmpeg` FFI (remux).
///
/// The original file is replaced with the post-processed result.
/// Operations applied (in order):
/// 1. Audio extraction (if `audio_only`)
/// 2. Subtitle embedding
/// 3. Chapter embedding
/// 4. Cover image embedding
/// 5. Metadata embedding
pub async fn postprocess(
    media_file: &Path,
    options: &PostProcessOptions,
) -> Result<PathBuf, String> {
    if !options.has_work() {
        return Ok(media_file.to_path_buf());
    }

    if options.audio_only {
        return extract_audio(media_file, options).await;
    }

    let temp_output = media_file.with_extension("_pp.tmp.mp4");

    // For faststart-only, prefer the system ffmpeg subprocess. It is more
    // battle-tested than our FFI remux for exotic codec/container combinations
    // (e.g. AV1 + OPUS produced by yt-dlp). Fall back to FFI if unavailable.
    if options.is_faststart_only() {
        // yt-dlp (with a system ffmpeg in PATH) already produces faststart-
        // compatible MP4 files (moov before mdat).  Re-muxing such a file is
        // a no-op at best and corrupting at worst — skip the whole step.
        if mp4_is_faststart(media_file).await {
            info!(path = %media_file.display(), "file is already faststart, skipping postprocess");
            return Ok(media_file.to_path_buf());
        }

        match run_faststart_subprocess(media_file, &temp_output).await {
            Ok(()) => {
                tokio::fs::rename(&temp_output, media_file)
                    .await
                    .map_err(|err| {
                        format!("failed to replace media with faststart output: {err}")
                    })?;
                info!(path = %media_file.display(), "faststart applied via subprocess ffmpeg");
                return Ok(media_file.to_path_buf());
            }
            Err(err) => {
                warn!(%err, "subprocess ffmpeg faststart failed, falling back to FFI remux");
                let _ = tokio::fs::remove_file(&temp_output).await;
            }
        }
    }

    let input = media_file.to_path_buf();
    let output = temp_output.clone();
    let opts = options.clone();

    tokio::task::spawn_blocking(move || {
        use tokimo_package_ffmpeg::remux::{self, RemuxOptions};

        remux::remux(&RemuxOptions {
            input,
            output,
            subtitle_file: opts.subtitle_file,
            subtitle_language: opts.subtitle_language,
            chapters_file: opts.chapters_file,
            cover_file: opts.cover_file,
            metadata: opts.metadata,
            strip_video: false,
            movflags_faststart: true,
        })
        .map_err(|e| format!("FFI remux failed: {e}"))
    })
    .await
    .map_err(|e| format!("task join error: {e}"))??;

    // Replace original with post-processed file
    tokio::fs::rename(&temp_output, media_file)
        .await
        .map_err(|err| format!("failed to replace media with postprocessed output: {err}"))?;

    info!(path = %media_file.display(), "post-processing complete");
    Ok(media_file.to_path_buf())
}

/// Returns true if the MP4 file already has its `moov` atom before `mdat`
/// (i.e. it is already "faststart" compatible).
///
/// Reads only the first few box headers (< 100 bytes for a typical MP4) so
/// this check is effectively instantaneous.
async fn mp4_is_faststart(path: &Path) -> bool {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    let Ok(mut file) = tokio::fs::File::open(path).await else {
        return false;
    };

    let mut buf = [0u8; 8];
    let mut offset: u64 = 0;

    for _ in 0..8 {
        if file.read_exact(&mut buf).await.is_err() {
            break;
        }
        let size = u64::from(u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]));
        let name = &buf[4..8];

        if name == b"moov" {
            return true;
        }
        if name == b"mdat" {
            return false;
        }
        if size < 8 {
            break;
        }
        offset += size;
        if file.seek(tokio::io::SeekFrom::Start(offset)).await.is_err() {
            break;
        }
    }
    false
}

/// Run `ffmpeg -y -i input -c copy -movflags +faststart output` as a subprocess.
///
/// This is preferred over the FFI remux for the faststart-only case because it
/// relies on the well-tested system ffmpeg code path rather than our custom muxing
/// logic, which can produce unexpected output for some codec/container combinations.
async fn run_faststart_subprocess(input: &Path, output: &Path) -> Result<(), String> {
    let Some(input_str) = input.to_str() else {
        return Err("input path contains non-UTF-8 characters".into());
    };
    let Some(output_str) = output.to_str() else {
        return Err("output path contains non-UTF-8 characters".into());
    };

    let status = tokio::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-i",
            input_str,
            "-c",
            "copy",
            "-movflags",
            "+faststart",
            output_str,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map_err(|e| format!("failed to spawn ffmpeg subprocess: {e}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("ffmpeg subprocess exited with status {status}"))
    }
}

/// Extract audio-only track from a media file.
async fn extract_audio(media_file: &Path, options: &PostProcessOptions) -> Result<PathBuf, String> {
    let ext = options
        .audio_container
        .as_deref()
        .or_else(|| media_file.extension().and_then(|value| value.to_str()))
        .unwrap_or("m4a");
    let output_path = media_file.with_extension(ext);
    let temp_output = if output_path == media_file {
        media_file.with_extension(format!("_pp.tmp.{ext}"))
    } else {
        output_path.clone()
    };

    let input = media_file.to_path_buf();
    let out = temp_output.clone();
    let metadata = options.metadata.clone();

    tokio::task::spawn_blocking(move || {
        tokimo_package_ffmpeg::remux::extract_audio(&input, &out, &metadata)
            .map_err(|e| format!("FFI audio extraction failed: {e}"))
    })
    .await
    .map_err(|e| format!("task join error: {e}"))??;

    if temp_output != output_path {
        tokio::fs::remove_file(&output_path).await.map_err(|err| {
            format!("failed to replace original media during audio extraction: {err}")
        })?;
        tokio::fs::rename(&temp_output, &output_path)
            .await
            .map_err(|err| format!("failed to finalize extracted audio output: {err}"))?;
    }

    // Remove the original video file since we only want audio
    if output_path != media_file {
        let _ = tokio::fs::remove_file(media_file).await;
    }

    info!(path = %output_path.display(), "audio extraction complete");
    Ok(output_path)
}

/// Generate an ffmetadata chapter file from chapter mark data.
pub async fn write_chapters_file(
    staging_dir: &Path,
    chapters: &[ChapterMark],
) -> Result<PathBuf, String> {
    let mut content = String::from(";FFMETADATA1\n\n");
    for chapter in chapters {
        let start_ms = (chapter.start_seconds * 1000.0) as u64;
        let end_ms = (chapter.end_seconds * 1000.0) as u64;
        content.push_str("[CHAPTER]\n");
        content.push_str("TIMEBASE=1/1000\n");
        let _ = writeln!(content, "START={start_ms}");
        let _ = writeln!(content, "END={end_ms}");
        let _ = write!(content, "title={}\n\n", chapter.title);
    }

    let path = staging_dir.join("_chapters.ffmeta");
    tokio::fs::write(&path, content.as_bytes())
        .await
        .map_err(|err| format!("failed to write chapters file: {err}"))?;
    Ok(path)
}

/// A chapter mark with start/end time and title.
#[derive(Debug, Clone)]
pub struct ChapterMark {
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub title: String,
}

/// Parse chapter marks from `raw_metadata` JSON (yt-dlp / provider format).
///
/// Expected format: `{ "chapters": [{ "start_time": 0.0, "end_time": 120.0, "title": "Intro" }, ...] }`
pub fn parse_chapters_from_metadata(raw_metadata: &serde_json::Value) -> Vec<ChapterMark> {
    let Some(chapters_array) = raw_metadata.get("chapters").and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    chapters_array
        .iter()
        .filter_map(|entry| {
            let start = entry
                .get("start_time")
                .and_then(serde_json::Value::as_f64)?;
            let end = entry.get("end_time").and_then(serde_json::Value::as_f64)?;
            let title = entry
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("Chapter")
                .to_string();
            Some(ChapterMark {
                start_seconds: start,
                end_seconds: end,
                title,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_chapters_from_metadata() {
        let metadata = serde_json::json!({
            "chapters": [
                { "start_time": 0.0, "end_time": 60.0, "title": "Intro" },
                { "start_time": 60.0, "end_time": 180.5, "title": "Main Content" },
                { "start_time": 180.5, "end_time": 240.0, "title": "Outro" }
            ]
        });

        let chapters = parse_chapters_from_metadata(&metadata);
        assert_eq!(chapters.len(), 3);
        assert_eq!(chapters[0].title, "Intro");
        assert!((chapters[0].start_seconds - 0.0).abs() < f64::EPSILON);
        assert!((chapters[0].end_seconds - 60.0).abs() < f64::EPSILON);
        assert_eq!(chapters[1].title, "Main Content");
        assert!((chapters[2].end_seconds - 240.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_parse_chapters_empty() {
        let metadata = serde_json::json!({ "title": "No chapters" });
        let chapters = parse_chapters_from_metadata(&metadata);
        assert!(chapters.is_empty());
    }

    #[test]
    fn test_postprocess_options_has_work() {
        assert!(!PostProcessOptions::default().has_work());

        let with_sub = PostProcessOptions {
            subtitle_file: Some(PathBuf::from("sub.srt")),
            ..Default::default()
        };
        assert!(with_sub.has_work());

        let audio_only = PostProcessOptions {
            audio_only: true,
            ..Default::default()
        };
        assert!(audio_only.has_work());

        let with_meta = PostProcessOptions {
            metadata: vec![("title".into(), "Test".into())],
            ..Default::default()
        };
        assert!(with_meta.has_work());
    }
}
