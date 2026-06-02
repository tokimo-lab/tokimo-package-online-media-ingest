use std::path::{Path, PathBuf};

use reqwest::header::{CONTENT_TYPE, REFERER, USER_AGENT};
use tokio::fs;
use url::Url;

use crate::models::{AnalyzeOnlineMediaResponse, CreateTaskRequest, OutputFile};
use crate::tooling::display_path;

const THUMBNAIL_ARTIFACT_DIR: &str = ".artifacts";
const THUMBNAIL_FILE_STEM: &str = "thumbnail";
const THUMBNAIL_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/136.0.0.0 Safari/537.36";

#[derive(Debug, Clone)]
pub struct DownloadedThumbnail {
    pub path: PathBuf,
    pub extension: String,
    pub mime_type: String,
}

fn normalize_extension(extension: &str) -> String {
    match extension.to_ascii_lowercase().as_str() {
        "jpeg" => "jpg".into(),
        other => other.into(),
    }
}

fn image_extension_from_content_type(content_type: &str) -> Option<&'static str> {
    match content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "image/jpeg" => Some("jpg"),
        "image/png" => Some("png"),
        "image/webp" => Some("webp"),
        "image/gif" => Some("gif"),
        "image/bmp" => Some("bmp"),
        "image/avif" => Some("avif"),
        _ => None,
    }
}

fn image_extension_from_url(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    let path = parsed.path();
    let extension = Path::new(path).extension()?.to_str()?;
    let extension = normalize_extension(extension);
    is_image_extension(&extension).then_some(extension)
}

fn image_mime_type_from_extension(extension: &str) -> &'static str {
    match normalize_extension(extension).as_str() {
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "avif" => "image/avif",
        _ => "image/jpeg",
    }
}

fn resolve_thumbnail_extension(content_type: Option<&str>, url: &str) -> String {
    content_type
        .and_then(image_extension_from_content_type)
        .map(str::to_string)
        .or_else(|| image_extension_from_url(url))
        .unwrap_or_else(|| "jpg".into())
}

fn resolve_thumbnail_url(
    request: &CreateTaskRequest,
    analyze: &AnalyzeOnlineMediaResponse,
) -> Option<String> {
    request
        .metadata
        .thumbnail_url
        .clone()
        .or_else(|| analyze.thumbnail_url.clone())
}

pub fn is_image_extension(extension: &str) -> bool {
    matches!(
        normalize_extension(extension).as_str(),
        "jpg" | "png" | "webp" | "gif" | "bmp" | "avif"
    )
}

pub fn is_media_sidecar_target(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    let extension = normalize_extension(extension);

    !matches!(extension.as_str(), "json" | "nfo" | "part" | "tmp" | "ytdl")
        && !is_image_extension(&extension)
}

pub async fn download_thumbnail_artifact(
    staging_dir: &Path,
    request: &CreateTaskRequest,
    analyze: &AnalyzeOnlineMediaResponse,
) -> Result<Option<DownloadedThumbnail>, String> {
    let Some(thumbnail_url) = resolve_thumbnail_url(request, analyze) else {
        return Ok(None);
    };

    let client = reqwest::Client::builder()
        .build()
        .map_err(|err| format!("failed to build thumbnail http client: {err}"))?;
    let referer = request.normalized_url.as_deref().unwrap_or(&request.url);
    let response = client
        .get(&thumbnail_url)
        .header(USER_AGENT, THUMBNAIL_USER_AGENT)
        .header(REFERER, referer)
        .send()
        .await
        .map_err(|err| format!("failed to request thumbnail {thumbnail_url}: {err}"))?
        .error_for_status()
        .map_err(|err| format!("thumbnail request failed for {thumbnail_url}: {err}"))?;
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let extension = resolve_thumbnail_extension(content_type.as_deref(), &thumbnail_url);
    let bytes = response
        .bytes()
        .await
        .map_err(|err| format!("failed to read thumbnail {thumbnail_url}: {err}"))?;
    if bytes.is_empty() {
        return Err(format!(
            "thumbnail request returned empty body for {thumbnail_url}"
        ));
    }

    let asset_dir = staging_dir.join(THUMBNAIL_ARTIFACT_DIR);
    fs::create_dir_all(&asset_dir).await.map_err(|err| {
        format!(
            "failed to create thumbnail artifact dir {}: {err}",
            display_path(&asset_dir)
        )
    })?;

    let path = asset_dir.join(format!("{THUMBNAIL_FILE_STEM}.{extension}"));
    fs::write(&path, &bytes)
        .await
        .map_err(|err| format!("failed to write thumbnail {}: {err}", display_path(&path)))?;

    Ok(Some(DownloadedThumbnail {
        path,
        extension: extension.clone(),
        mime_type: image_mime_type_from_extension(&extension).into(),
    }))
}

pub async fn write_thumbnail_sidecar_files(
    target_dir: &Path,
    imported_files: &mut Vec<OutputFile>,
    thumbnail: &DownloadedThumbnail,
) -> Result<(), String> {
    let media_files: Vec<_> = imported_files
        .iter()
        .filter_map(|file| {
            let path = Path::new(&file.path);
            is_media_sidecar_target(path).then(|| path.to_path_buf())
        })
        .collect();

    let mut created_paths = Vec::new();
    for media_path in media_files {
        let Some(stem) = media_path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        let destination_path = target_dir.join(format!("{stem}.{}", thumbnail.extension));
        if created_paths
            .iter()
            .any(|path: &PathBuf| path == &destination_path)
        {
            continue;
        }

        fs::copy(&thumbnail.path, &destination_path)
            .await
            .map_err(|err| {
                format!(
                    "failed to copy thumbnail {} to {}: {err}",
                    display_path(&thumbnail.path),
                    display_path(&destination_path),
                )
            })?;

        let metadata = fs::metadata(&destination_path).await.map_err(|err| {
            format!(
                "failed to stat imported thumbnail {}: {err}",
                display_path(&destination_path)
            )
        })?;
        imported_files.push(OutputFile {
            path: display_path(&destination_path),
            size_bytes: Some(metadata.len()),
            mime_type: Some(thumbnail.mime_type.clone()),
        });
        created_paths.push(destination_path);
    }

    imported_files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{is_image_extension, is_media_sidecar_target, resolve_thumbnail_extension};

    #[test]
    fn prefers_extension_from_content_type() {
        assert_eq!(
            resolve_thumbnail_extension(
                Some("image/png; charset=binary"),
                "https://example.com/poster.jpg"
            ),
            "png"
        );
    }

    #[test]
    fn falls_back_to_url_extension() {
        assert_eq!(
            resolve_thumbnail_extension(None, "https://example.com/poster.jpeg?size=large"),
            "jpg"
        );
    }

    #[test]
    fn excludes_image_files_from_media_sidecars() {
        assert!(!is_media_sidecar_target(Path::new("video.jpg")));
        assert!(!is_media_sidecar_target(Path::new("video.nfo")));
        assert!(is_media_sidecar_target(Path::new("video.mp4")));
        assert!(is_image_extension("jpeg"));
    }
}
