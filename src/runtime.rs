use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tokio::fs;
use tracing::{info, warn};

use crate::AppState;
use crate::models::{AnalyzeOnlineMediaRequest, AnalyzeOnlineMediaResponse, CreateTaskRequest, Manifest, OutputFile};
use crate::music::{
    MusicMetadata, extract_music_metadata, is_music_content_type, music_metadata_from_analysis,
    music_metadata_from_task,
};
use crate::native::ytdlp_download;
use crate::providers::analyze_url;
use crate::thumbnail::{DownloadedThumbnail, download_thumbnail_artifact, write_thumbnail_sidecar_files};
use crate::tooling::display_path;

fn sanitize_path_component(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .to_string();

    if sanitized.is_empty() {
        "online-media".into()
    } else {
        sanitized
    }
}

fn build_import_dir_name(request: &CreateTaskRequest) -> String {
    let title = request
        .metadata
        .media_title
        .clone()
        .or(request.metadata.title.clone())
        .unwrap_or_else(|| request.record_id.clone());
    let title = sanitize_path_component(&title);

    if let Some(external_id) = request.metadata.external_id.clone() {
        let external_id = sanitize_path_component(&external_id);
        format!("{title} [{external_id}]")
    } else {
        title
    }
}

fn build_source_site_dir_name(request: &CreateTaskRequest) -> Option<String> {
    request.metadata.source_site.as_deref().map(sanitize_path_component)
}

fn build_source_id_dir_name(request: &CreateTaskRequest) -> Option<String> {
    request
        .metadata
        .source_id
        .clone()
        .or(request.metadata.external_id.clone())
        .map(|value| sanitize_path_component(&value))
}

fn is_music_request(request: &CreateTaskRequest) -> bool {
    is_music_content_type(&request.target_folder_config_snapshot.content_type)
}

fn should_rename_music_output(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };

    matches!(
        extension.to_ascii_lowercase().as_str(),
        "m4a" | "mp3" | "flac" | "ogg" | "opus" | "wav" | "aac" | "wma" | "ape"
    )
}

fn merged_music_metadata(request: &CreateTaskRequest, analyze: &AnalyzeOnlineMediaResponse) -> MusicMetadata {
    let task_metadata = music_metadata_from_task(&request.metadata);
    let analyze_metadata = music_metadata_from_analysis(analyze);
    let raw_metadata = extract_music_metadata(request.metadata.raw_metadata.as_ref().or(analyze.raw_metadata.as_ref()));

    task_metadata.merge_with(&analyze_metadata).merge_with(&raw_metadata)
}

fn build_music_target_dir(
    target_root: &Path,
    base_name: &str,
    music_metadata: &MusicMetadata,
    fallback_artist: Option<&str>,
) -> PathBuf {
    let artist = music_metadata
        .album_artist
        .as_deref()
        .or(music_metadata.artist.as_deref())
        .or(fallback_artist)
        .map(sanitize_path_component);
    let album = music_metadata.album.as_deref().map(sanitize_path_component);
    let year = music_metadata.year();

    match (artist, album) {
        (Some(artist), Some(album)) => {
            if let Some(year) = year {
                target_root.join(artist).join(format!("{album} ({year})"))
            } else {
                target_root.join(artist).join(album)
            }
        }
        (Some(artist), None) => target_root.join(artist),
        (None, Some(album)) => target_root.join(album),
        (None, None) => target_root.join(base_name),
    }
}

fn build_music_output_name(path: &Path, music_metadata: &MusicMetadata) -> Option<String> {
    if !should_rename_music_output(path) {
        return None;
    }

    let extension = path.extension().and_then(|ext| ext.to_str())?;
    let title = music_metadata.title.as_deref().map(sanitize_path_component)?;

    let stem = if let Some(track_number) = music_metadata.track_number {
        format!("{track_number:02}. {title}")
    } else {
        title
    };

    Some(format!("{stem}.{extension}"))
}

fn with_record_id_suffix(path: &Path, record_id: &str) -> PathBuf {
    let stem = path.file_stem().and_then(|value| value.to_str()).unwrap_or("output");
    let suffix = format!("{} [{}]", stem, sanitize_path_component(record_id));
    match path.extension().and_then(|ext| ext.to_str()) {
        Some(extension) => path.with_file_name(format!("{suffix}.{extension}")),
        None => path.with_file_name(suffix),
    }
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn xml_tag(name: &str, value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }

    Some(format!("  <{name}>{}</{name}>", escape_xml(value)))
}

fn xml_tag_number<T: ToString>(name: &str, value: Option<T>) -> Option<String> {
    value.map(|value| format!("  <{name}>{}</{name}>", escape_xml(&value.to_string())))
}

fn metadata_string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str()
}

fn metadata_release_date(raw_metadata: Option<&Value>) -> Option<String> {
    let upload_date = metadata_string_field(raw_metadata?, "upload_date")?;
    if upload_date.len() != 8 || !upload_date.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }

    Some(format!(
        "{}-{}-{}",
        &upload_date[0..4],
        &upload_date[4..6],
        &upload_date[6..8]
    ))
}

fn build_online_media_nfo_content(request: &CreateTaskRequest, analyze: &AnalyzeOnlineMediaResponse) -> String {
    let raw_metadata = request.metadata.raw_metadata.as_ref().or(analyze.raw_metadata.as_ref());
    let title = request
        .metadata
        .media_title
        .as_deref()
        .or(request.metadata.title.as_deref())
        .or(analyze.title.as_deref())
        .unwrap_or("Online Media");
    let original_title = request.metadata.title.as_deref().or(analyze.title.as_deref());
    let plot = raw_metadata
        .and_then(|value| metadata_string_field(value, "description"))
        .or(raw_metadata.and_then(|value| metadata_string_field(value, "fulltitle")));
    let release_date = metadata_release_date(raw_metadata);
    let year = request
        .metadata
        .media_year
        .as_deref()
        .map(str::to_owned)
        .or_else(|| release_date.as_deref().map(|value| value[0..4].to_string()));
    let thumbnail_url = request
        .metadata
        .thumbnail_url
        .as_deref()
        .or(analyze.thumbnail_url.as_deref());
    let source_site = request
        .metadata
        .source_site
        .as_deref()
        .or(analyze.source_site.as_deref());
    let source_id = request
        .metadata
        .source_id
        .as_deref()
        .or(analyze.source_id.as_deref())
        .or(request.metadata.external_id.as_deref())
        .or(analyze.external_id.as_deref());
    let uploader = request.metadata.uploader.as_deref().or(analyze.uploader.as_deref());
    let external_id = request
        .metadata
        .external_id
        .as_deref()
        .or(analyze.external_id.as_deref());
    let runtime = request.metadata.duration_seconds.or(analyze.duration_seconds);

    let mut lines = vec![
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>".to_string(),
        "<movie>".to_string(),
        format!("  <title>{}</title>", escape_xml(title)),
    ];

    if let Some(line) = xml_tag("originaltitle", original_title) {
        lines.push(line);
    }
    if let Some(line) = xml_tag("sorttitle", Some(title)) {
        lines.push(line);
    }
    if let Some(plot) = plot {
        lines.push(format!("  <plot><![CDATA[{plot}]]></plot>"));
    }
    if let Some(line) = xml_tag("studio", source_site) {
        lines.push(line);
    }
    if let Some(line) = xml_tag("director", uploader) {
        lines.push(line);
    }
    if let Some(line) = xml_tag_number("runtime", runtime) {
        lines.push(line);
    }
    if let Some(year) = year.as_deref()
        && let Some(line) = xml_tag("year", Some(year))
    {
        lines.push(line);
    }
    if let Some(premiered) = release_date.as_deref()
        && let Some(line) = xml_tag("premiered", Some(premiered))
    {
        lines.push(line);
    }
    if let Some(provider_id) = request.provider_id.as_deref() {
        lines.push(format!(
            "  <uniqueid type=\"provider\" default=\"true\">{}</uniqueid>",
            escape_xml(provider_id)
        ));
    }
    if let Some(source_id) = source_id {
        let unique_id_type = if request.provider_id.as_deref() == Some("bilibili") {
            "bilibili"
        } else {
            request.provider_id.as_deref().unwrap_or("source")
        };
        lines.push(format!(
            "  <uniqueid type=\"{}\">{}</uniqueid>",
            escape_xml(unique_id_type),
            escape_xml(source_id)
        ));
    }
    if let Some(external_id) = external_id
        && Some(external_id) != source_id
    {
        lines.push(format!(
            "  <uniqueid type=\"external\">{}</uniqueid>",
            escape_xml(external_id)
        ));
    }
    if let Some(line) = xml_tag("sourceurl", Some(&request.url)) {
        lines.push(line);
    }
    if let Some(thumb) = thumbnail_url {
        lines.push(format!("  <thumb aspect=\"poster\">{}</thumb>", escape_xml(thumb)));
    }
    lines.push("</movie>".to_string());
    format!("{}\n", lines.join("\n"))
}

fn should_generate_nfo(request: &CreateTaskRequest) -> bool {
    if is_music_request(request) {
        return false;
    }

    request.metadata.generate_nfo.unwrap_or(true)
}

fn should_emit_sidecar_nfo(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };

    !matches!(
        extension.to_ascii_lowercase().as_str(),
        "json" | "nfo" | "part" | "tmp" | "ytdl"
    )
}

async fn write_sidecar_nfo_files(
    request: &CreateTaskRequest,
    analyze: &AnalyzeOnlineMediaResponse,
    target_dir: &Path,
    imported_files: &mut Vec<OutputFile>,
) -> Result<(), String> {
    if !should_generate_nfo(request) {
        return Ok(());
    }

    let nfo_content = build_online_media_nfo_content(request, analyze);
    let media_files: Vec<_> = imported_files
        .iter()
        .filter_map(|file| {
            let path = Path::new(&file.path);
            should_emit_sidecar_nfo(path).then(|| path.to_path_buf())
        })
        .collect();

    for media_path in media_files {
        let Some(stem) = media_path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };

        let nfo_path = target_dir.join(format!("{stem}.nfo"));
        fs::write(&nfo_path, nfo_content.as_bytes())
            .await
            .map_err(|err| format!("failed to write nfo {}: {err}", display_path(&nfo_path)))?;

        let metadata = fs::metadata(&nfo_path)
            .await
            .map_err(|err| format!("failed to stat nfo {}: {err}", display_path(&nfo_path)))?;
        imported_files.push(OutputFile {
            path: display_path(&nfo_path),
            size_bytes: Some(metadata.len()),
            mime_type: Some("application/xml".into()),
        });
    }

    imported_files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(())
}

fn build_download_workspace_dir(state: &AppState, request: &CreateTaskRequest, task_id: &str) -> std::path::PathBuf {
    // Always use the local staging_root for the temporary download workspace,
    // regardless of whether target_path is absolute (e.g. an SMB/SFTP mount).
    // yt-dlp requires a writable local directory; the final output is copied to
    // the real target in copy_outputs_to_target() after the download completes.
    state.staging_root.join(&request.record_id).join(task_id)
}

async fn collect_output_files(dir: &Path) -> Result<Vec<OutputFile>, String> {
    let mut entries = fs::read_dir(dir)
        .await
        .map_err(|err| format!("failed to read staging dir: {err}"))?;
    let mut files = Vec::new();

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|err| format!("failed to iterate staging dir: {err}"))?
    {
        let file_type = entry
            .file_type()
            .await
            .map_err(|err| format!("failed to inspect entry: {err}"))?;
        if !file_type.is_file() {
            continue;
        }

        let path = entry.path();
        let metadata = entry
            .metadata()
            .await
            .map_err(|err| format!("failed to read file metadata: {err}"))?;
        files.push(OutputFile {
            path: path.to_string_lossy().into_owned(),
            size_bytes: Some(metadata.len()),
            mime_type: None,
        });
    }

    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

async fn copy_outputs_to_target(
    request: &CreateTaskRequest,
    analyze: &AnalyzeOnlineMediaResponse,
    staging_dir: &Path,
) -> Result<(String, Vec<OutputFile>), String> {
    let target_root = Path::new(&request.target_folder_config_snapshot.target_path);
    fs::create_dir_all(target_root)
        .await
        .map_err(|err| format!("failed to create target root: {err}"))?;

    let base_name = build_import_dir_name(request);
    let music_metadata = merged_music_metadata(request, analyze);
    let mut target_dir = if is_music_request(request) {
        build_music_target_dir(
            target_root,
            &base_name,
            &music_metadata,
            request.metadata.uploader.as_deref().or(analyze.uploader.as_deref()),
        )
    } else if request.target_folder_config_snapshot.content_type == "online_video" {
        if let (Some(source_site), Some(source_id)) =
            (build_source_site_dir_name(request), build_source_id_dir_name(request))
        {
            target_root.join(source_site).join(source_id)
        } else {
            target_root.join(&base_name)
        }
    } else {
        target_root.join(&base_name)
    };
    if !is_music_request(request)
        && request.target_folder_config_snapshot.content_type != "online_video"
        && fs::try_exists(&target_dir).await.unwrap_or(false)
    {
        target_dir = target_root.join(format!("{base_name} [{}]", request.record_id));
    }
    if is_music_request(request)
        && target_dir == target_root.join(&base_name)
        && fs::try_exists(&target_dir).await.unwrap_or(false)
    {
        target_dir = target_root.join(format!("{base_name} [{}]", request.record_id));
    }

    fs::create_dir_all(&target_dir)
        .await
        .map_err(|err| format!("failed to create target dir: {err}"))?;

    let mut entries = fs::read_dir(staging_dir)
        .await
        .map_err(|err| format!("failed to read staging dir for import: {err}"))?;
    let mut copied_files = Vec::new();

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|err| format!("failed to iterate staging outputs: {err}"))?
    {
        let file_type = entry
            .file_type()
            .await
            .map_err(|err| format!("failed to inspect staging entry: {err}"))?;
        if !file_type.is_file() {
            continue;
        }

        let source_path = entry.path();
        let Some(file_name) = source_path.file_name() else {
            continue;
        };
        let mut destination_path = if is_music_request(request) {
            let destination_name = build_music_output_name(&source_path, &music_metadata)
                .unwrap_or_else(|| file_name.to_string_lossy().into_owned());
            target_dir.join(destination_name)
        } else {
            target_dir.join(file_name)
        };
        if fs::try_exists(&destination_path).await.unwrap_or(false) {
            destination_path = with_record_id_suffix(&destination_path, &request.record_id);
        }
        fs::copy(&source_path, &destination_path).await.map_err(|err| {
            format!(
                "failed to copy {} to {}: {err}",
                display_path(&source_path),
                display_path(&destination_path),
            )
        })?;

        let metadata = fs::metadata(&destination_path)
            .await
            .map_err(|err| format!("failed to stat imported file: {err}"))?;
        copied_files.push(OutputFile {
            path: display_path(&destination_path),
            size_bytes: Some(metadata.len()),
            mime_type: None,
        });
    }

    copied_files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok((display_path(&target_dir), copied_files))
}

async fn write_manifest(
    staging_dir: &Path,
    request: &CreateTaskRequest,
    analyze: &AnalyzeOnlineMediaResponse,
    output_files: Vec<OutputFile>,
) -> Result<String, String> {
    let raw_metadata = request.metadata.raw_metadata.as_ref().or(analyze.raw_metadata.as_ref());
    let music_metadata = merged_music_metadata(request, analyze);

    let manifest = Manifest {
        source_url: request.url.clone(),
        provider_id: request.provider_id.clone().unwrap_or_else(|| "unknown".into()),
        source_site: request.metadata.source_site.clone().or(analyze.source_site.clone()),
        source_id: request
            .metadata
            .source_id
            .clone()
            .or(analyze.source_id.clone())
            .or(request.metadata.external_id.clone())
            .or(analyze.external_id.clone()),
        title: request
            .metadata
            .track_title
            .clone()
            .or(music_metadata.title.clone())
            .or(request.metadata.media_title.clone())
            .or(request.metadata.title.clone())
            .or(analyze.track_title.clone())
            .or(analyze.title.clone()),
        original_title: request.metadata.title.clone().or(analyze.title.clone()),
        thumbnail_url: request.metadata.thumbnail_url.clone().or(analyze.thumbnail_url.clone()),
        description: raw_metadata
            .and_then(|value| metadata_string_field(value, "description"))
            .map(str::to_string),
        duration_seconds: request.metadata.duration_seconds.or(analyze.duration_seconds),
        uploader: request.metadata.uploader.clone().or(analyze.uploader.clone()),
        artist: music_metadata.artist.clone(),
        album_artist: music_metadata.album_artist.clone(),
        album: music_metadata.album.clone(),
        track_title: music_metadata.title.clone(),
        track_number: music_metadata.track_number,
        disc_number: music_metadata.disc_number,
        genre: music_metadata.genre.clone(),
        release_date: music_metadata
            .release_date
            .clone()
            .or_else(|| metadata_release_date(raw_metadata)),
        content_type: Some(request.target_folder_config_snapshot.content_type.clone()),
        external_id: request.metadata.external_id.clone().or(analyze.external_id.clone()),
        output_files,
        artifacts: vec![],
        raw_metadata: request
            .metadata
            .raw_metadata
            .clone()
            .or(analyze.raw_metadata.clone())
            .or_else(|| {
                Some(json!({
                    "warnings": analyze.warnings.clone(),
                    "normalizedUrl": request.normalized_url,
                }))
            }),
    };

    let manifest_path = staging_dir.join("manifest.json");
    let bytes = serde_json::to_vec_pretty(&manifest).map_err(|err| format!("failed to serialize manifest: {err}"))?;
    fs::write(&manifest_path, bytes)
        .await
        .map_err(|err| format!("failed to write manifest: {err}"))?;

    Ok(manifest_path.to_string_lossy().into_owned())
}

pub fn spawn_task(state: AppState, task_id: String, request: CreateTaskRequest) {
    tokio::spawn(async move {
        state.tasks.update_stage(&task_id, "preparing", 5.0).await;

        let staging_dir = build_download_workspace_dir(&state, &request, task_id.as_str());

        if let Err(err) = fs::create_dir_all(&staging_dir).await {
            state
                .tasks
                .fail(
                    &task_id,
                    format!(
                        "failed to create download workspace {}: {err}",
                        display_path(&staging_dir),
                    ),
                )
                .await;
            return;
        }

        state.tasks.update_stage(&task_id, "analyzing", 15.0).await;
        let analyze = analyze_url(&AnalyzeOnlineMediaRequest {
            url: request.url.clone(),
            target_library_id: Some(request.target_library_id.clone()),
            preferred_provider: request.provider_id.clone(),
        })
        .await;
        if !analyze.warnings.is_empty() {
            warn!(task_id = %task_id, warnings = ?analyze.warnings, "analyze warnings");
        }

        state.tasks.update_stage(&task_id, "downloading", 0.0).await;
        match ytdlp_download(&state, &task_id, &request, &staging_dir).await {
            Ok(()) => {
                state.tasks.update_stage(&task_id, "packaging", 85.0).await;
            }
            Err(err) => {
                state.tasks.fail(&task_id, err).await;
                return;
            }
        }

        let downloaded_thumbnail: Option<DownloadedThumbnail> =
            match download_thumbnail_artifact(&staging_dir, &request, &analyze).await {
                Ok(result) => result,
                Err(err) => {
                    warn!(task_id = %task_id, error = %err, "thumbnail download failed");
                    None
                }
            };

        let output_files = match collect_output_files(&staging_dir).await {
            Ok(files) => files,
            Err(err) => {
                state.tasks.fail(&task_id, err).await;
                return;
            }
        };

        state.tasks.update_stage(&task_id, "importing", 92.0).await;
        let (target_path, mut imported_files) = match copy_outputs_to_target(&request, &analyze, &staging_dir).await {
            Ok(result) => result,
            Err(err) => {
                state.tasks.fail(&task_id, err).await;
                return;
            }
        };

        if let Some(thumbnail) = downloaded_thumbnail.as_ref()
            && let Err(err) =
                write_thumbnail_sidecar_files(Path::new(&target_path), &mut imported_files, thumbnail).await
        {
            state.tasks.fail(&task_id, err).await;
            return;
        }

        if let Err(err) =
            write_sidecar_nfo_files(&request, &analyze, Path::new(&target_path), &mut imported_files).await
        {
            state.tasks.fail(&task_id, err).await;
            return;
        }

        let manifest_path = match write_manifest(&staging_dir, &request, &analyze, output_files.clone()).await {
            Ok(path) => path,
            Err(err) => {
                state.tasks.fail(&task_id, err).await;
                return;
            }
        };

        info!(task_id = %task_id, manifest_path = %manifest_path, staging_dir = %staging_dir.to_string_lossy(), "online media task completed in staging");
        state
            .tasks
            .complete(&task_id, Some(manifest_path), Some(target_path), imported_files)
            .await;
    });
}
