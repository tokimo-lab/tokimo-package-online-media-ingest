use tracing::warn;
use url::Url;

use crate::models::{
    AnalyzeOnlineMediaRequest, AnalyzeOnlineMediaResponse, CollectionItem, OnlineMediaCapability,
    OnlineMediaProvider, ResolveCollectionRequest, ResolveCollectionResponse,
};
use crate::music::merge_content_types;
use crate::native::{AnalyzeResult, ytdlp_analyze};
use crate::provider_catalog::{ProviderCatalogEntry, find_provider_by_host, find_provider_by_id};
use crate::tooling::resolve_ytdlp_binary;

#[derive(Debug, Clone)]
struct ProviderMatch {
    provider: OnlineMediaProvider,
    source_site: String,
    normalized_url: String,
    external_id: Option<String>,
    source_id: Option<String>,
    content_type: String,
}

fn extract_bilibili_bv_from_url(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    parsed.path_segments()?.find_map(|segment| {
        let normalized = segment.trim();
        normalized
            .strip_prefix("BV")
            .map(|suffix| format!("BV{suffix}"))
    })
}

fn match_catalog_provider(
    entry: &ProviderCatalogEntry,
    normalized_url: &str,
    external_id: Option<String>,
    source_id: Option<String>,
    content_type: Option<&str>,
) -> ProviderMatch {
    ProviderMatch {
        provider: entry.to_provider(),
        source_site: entry.source_site.clone(),
        normalized_url: normalized_url.into(),
        external_id,
        source_id,
        content_type: content_type.unwrap_or(&entry.default_content_type).into(),
    }
}

fn is_stream_url(url: &str) -> bool {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| {
            parsed
                .path_segments()
                .and_then(|mut segments| segments.next_back().map(str::to_string))
        })
        .is_some_and(|path| {
            let lower = path.to_ascii_lowercase();
            std::path::Path::new(&lower)
                .extension()
                .is_some_and(|ext| ext == "m3u8" || ext == "mpd")
        })
}

fn extract_youtube_video_id_from_url(parsed: &Url) -> Option<String> {
    if parsed.host_str()?.eq_ignore_ascii_case("youtu.be") {
        parsed
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .map(str::to_string)
    } else {
        parsed
            .query_pairs()
            .find(|(key, _)| key == "v")
            .map(|(_, value)| value.to_string())
    }
}

fn extract_source_id_from_url(parsed: &Url, raw_url: &str, provider_id: &str) -> Option<String> {
    match provider_id {
        "youtube" => extract_youtube_video_id_from_url(parsed),
        "bilibili" => extract_bilibili_bv_from_url(raw_url),
        _ => None,
    }
}

fn detect_provider(url: &str) -> Option<ProviderMatch> {
    if is_stream_url(url) {
        return find_provider_by_id("stream")
            .map(|entry| match_catalog_provider(entry, url, None, None, None));
    }

    let parsed = Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();

    let entry = find_provider_by_host(&host)?;
    let source_id = extract_source_id_from_url(&parsed, url, &entry.id);
    let external_id = source_id.clone();
    Some(match_catalog_provider(
        entry,
        url,
        external_id,
        source_id,
        None,
    ))
}

fn apply_ytdlp_analysis(response: &mut AnalyzeOnlineMediaResponse, result: AnalyzeResult) {
    response.normalized_url = Some(result.normalized_url);
    response.source_id = result.source_id.or(response.source_id.clone());
    response.external_id = result.external_id.or(response.external_id.clone());
    response.title = result.title;
    response.description = result.description;
    response.thumbnail_url = result.thumbnail_url;
    response.duration_seconds = result.duration_seconds;
    response.uploader = result.uploader;
    response.artist = result.artist.or(response.artist.clone());
    response.album_artist = result.album_artist.or(response.album_artist.clone());
    response.album = result.album.or(response.album.clone());
    response.track_title = result.track_title.or(response.track_title.clone());
    response.track_number = result.track_number.or(response.track_number);
    response.disc_number = result.disc_number.or(response.disc_number);
    response.genre = result.genre.or(response.genre.clone());
    response.release_date = result.release_date.or(response.release_date.clone());
    response.content_type = merge_content_types(response.content_type.clone(), result.content_type);
    response.requires_auth = result.requires_auth.unwrap_or(response.requires_auth);
    response.warnings.extend(result.warnings);
    response.raw_metadata = result.raw_metadata;
}

pub async fn analyze_url(input: &AnalyzeOnlineMediaRequest) -> AnalyzeOnlineMediaResponse {
    let matched = detect_provider(&input.url);
    let ytdlp_available = resolve_ytdlp_binary().is_some();

    let mut response = AnalyzeOnlineMediaResponse {
        is_supported: matched.is_some(),
        provider: matched.as_ref().map(|value| value.provider.clone()),
        capability: matched.as_ref().map(|_| OnlineMediaCapability {
            can_analyze: true,
            can_download: ytdlp_available,
            can_import_metadata: true,
            supports_collections: false,
        }),
        source_site: matched.as_ref().map(|value| value.source_site.clone()),
        source_id: matched.as_ref().and_then(|value| value.source_id.clone()),
        normalized_url: Some(
            matched
                .as_ref()
                .map_or_else(|| input.url.clone(), |value| value.normalized_url.clone()),
        ),
        title: None,
        description: None,
        thumbnail_url: None,
        duration_seconds: None,
        uploader: None,
        artist: None,
        album_artist: None,
        album: None,
        track_title: None,
        track_number: None,
        disc_number: None,
        genre: None,
        release_date: None,
        external_id: matched.as_ref().and_then(|value| value.external_id.clone()),
        content_type: matched.as_ref().map(|value| value.content_type.clone()),
        requires_auth: matched
            .as_ref()
            .is_some_and(|value| value.provider.requires_auth),
        warnings: Vec::new(),
        raw_metadata: None,
    };

    // All analysis goes through yt-dlp
    if ytdlp_available {
        match ytdlp_analyze(&input.url).await {
            Ok(result) => {
                if !response.is_supported {
                    response.is_supported = true;
                    response.provider = Some(OnlineMediaProvider {
                        id: "ytdlp".into(),
                        name: "yt-dlp".into(),
                        display_name: Some("yt-dlp".into()),
                        supported_content_types: vec!["online_video".into(), "music".into()],
                        requires_auth: false,
                    });
                    response.capability = Some(OnlineMediaCapability {
                        can_analyze: true,
                        can_download: true,
                        can_import_metadata: true,
                        supports_collections: false,
                    });
                }
                apply_ytdlp_analysis(&mut response, result);
            }
            Err(err) => {
                warn!(url = %input.url, error = %err, "yt-dlp analyze failed");
                response.warnings.push(err);
            }
        }
    } else {
        response
            .warnings
            .push("yt-dlp binary not found — download disabled".into());
    }

    if !response.is_supported {
        response.warnings.push("unsupported URL".into());
    }

    response
}

/// Resolve a collection (playlist / channel) URL into individual items via yt-dlp `--flat-playlist`.
pub async fn resolve_collection_url(
    input: &ResolveCollectionRequest,
) -> Result<ResolveCollectionResponse, String> {
    let ytdlp = resolve_ytdlp_binary().ok_or("yt-dlp binary not found")?;

    let mut args = vec![
        "--flat-playlist",
        "--dump-json",
        "--no-warnings",
        "--ignore-errors",
    ];

    // Write cookie file if auth provided
    let cookie_tempfile = if let Some(ref auth) = input.auth {
        if let Some(ref header) = auth.cookie_header {
            let tmp = std::env::temp_dir().join(format!("ytdlp_col_{}.txt", uuid::Uuid::new_v4()));
            crate::native::ytdlp::write_cookie_file(&tmp, &input.url, header)
                .await
                .map_err(|e| format!("failed to write cookie file: {e}"))?;
            Some(tmp)
        } else {
            None
        }
    } else {
        None
    };

    let cookie_path_str;
    if let Some(ref path) = cookie_tempfile {
        cookie_path_str = path.to_string_lossy().to_string();
        args.push("--cookies");
        args.push(&cookie_path_str);
    }

    // Apply limit via --playlist-end
    let limit_str;
    if let Some(limit) = input.limit {
        limit_str = limit.to_string();
        args.push("--playlist-end");
        args.push(&limit_str);
    }

    args.push(&input.url);

    let output = tokio::process::Command::new(&ytdlp)
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("failed to run yt-dlp: {e}"))?;

    // Cleanup cookie file
    if let Some(path) = cookie_tempfile {
        let _ = tokio::fs::remove_file(path).await;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    if stdout.trim().is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("yt-dlp returned no results: {stderr}"));
    }

    // Each line is a JSON object for one playlist entry
    let mut items = Vec::new();
    let mut collection_title: Option<String> = None;

    for (idx, line) in stdout.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };

        if collection_title.is_none() {
            collection_title = entry
                .get("playlist_title")
                .or_else(|| entry.get("playlist"))
                .and_then(|v| v.as_str())
                .map(String::from);
        }

        let url = entry
            .get("url")
            .or_else(|| entry.get("webpage_url"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if url.is_empty() {
            continue;
        }

        items.push(CollectionItem {
            url,
            title: entry
                .get("title")
                .and_then(|v| v.as_str())
                .map(String::from),
            thumbnail_url: entry
                .get("thumbnails")
                .and_then(|t| t.as_array())
                .and_then(|arr| arr.last())
                .and_then(|t| t.get("url"))
                .and_then(|v| v.as_str())
                .map(String::from),
            duration_seconds: entry.get("duration").and_then(serde_json::Value::as_u64),
            index: idx,
            source_id: entry.get("id").and_then(|v| v.as_str()).map(String::from),
        });
    }

    let provider_id = detect_provider(&input.url).map_or_else(|| "ytdlp".into(), |m| m.provider.id);

    let total = items.len();

    Ok(ResolveCollectionResponse {
        provider_id,
        collection_title,
        collection_url: input.url.clone(),
        total_items: Some(total),
        items,
        next_cursor: None,
    })
}
