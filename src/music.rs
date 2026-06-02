use serde_json::Value;

use crate::models::{AnalyzeOnlineMediaResponse, TaskMetadataInput};

#[derive(Debug, Clone, Default)]
pub struct MusicMetadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub genre: Option<String>,
    pub release_date: Option<String>,
}

impl MusicMetadata {
    #[must_use]
    pub fn merge_with(&self, fallback: &Self) -> Self {
        Self {
            title: self.title.clone().or(fallback.title.clone()),
            artist: self.artist.clone().or(fallback.artist.clone()),
            album_artist: self.album_artist.clone().or(fallback.album_artist.clone()),
            album: self.album.clone().or(fallback.album.clone()),
            track_number: self.track_number.or(fallback.track_number),
            disc_number: self.disc_number.or(fallback.disc_number),
            genre: self.genre.clone().or(fallback.genre.clone()),
            release_date: self.release_date.clone().or(fallback.release_date.clone()),
        }
    }

    pub fn has_signal(&self) -> bool {
        self.title.is_some()
            || self.artist.is_some()
            || self.album_artist.is_some()
            || self.album.is_some()
            || self.track_number.is_some()
            || self.genre.is_some()
    }

    pub fn year(&self) -> Option<String> {
        self.release_date
            .as_deref()
            .and_then(|value| value.get(0..4))
            .map(str::to_string)
    }
}

pub fn is_music_content_type(content_type: &str) -> bool {
    content_type.eq_ignore_ascii_case("music")
}

pub fn merge_content_types(existing: Option<String>, incoming: Option<String>) -> Option<String> {
    match (existing, incoming) {
        (None, None) => None,
        (Some(value), None) | (None, Some(value)) => Some(value),
        (Some(existing), Some(incoming)) => {
            if content_type_priority(&incoming) >= content_type_priority(&existing) {
                Some(incoming)
            } else {
                Some(existing)
            }
        }
    }
}

pub fn detect_ytdlp_content_type(raw_metadata: Option<&Value>) -> String {
    if has_strong_music_metadata(raw_metadata) || has_music_category(raw_metadata) || has_music_url_hint(raw_metadata) {
        "music".into()
    } else {
        "online_video".into()
    }
}

/// Check for music-specific fields in yt-dlp metadata.
///
/// Only considers fields that genuinely indicate music content (`track`,
/// `artist`, `album`, `album_artist`, `track_number`, `disc_number`).
/// Generic fields like `uploader`, `creator`, `playlist_title` and
/// `playlist_index` are present on virtually every video and must NOT
/// be treated as music signals.
fn has_strong_music_metadata(raw_metadata: Option<&Value>) -> bool {
    let Some(metadata) = raw_metadata else {
        return false;
    };

    let music_fields = [
        "track",
        "artist",
        "artists",
        "album_artist",
        "album",
        "track_number",
        "disc_number",
    ];

    music_fields.iter().any(|field| {
        metadata.get(field).is_some_and(|v| match v {
            Value::String(s) => !s.trim().is_empty(),
            Value::Number(_) => true,
            Value::Array(arr) => !arr.is_empty(),
            _ => false,
        })
    })
}

pub fn extract_music_metadata(raw_metadata: Option<&Value>) -> MusicMetadata {
    let Some(raw_metadata) = raw_metadata else {
        return MusicMetadata::default();
    };

    let album = first_string(raw_metadata, &["album", "playlist_title", "playlist"]);
    let title = first_string(raw_metadata, &["track"]);
    let artist = first_string(raw_metadata, &["artist", "artists", "creator", "uploader"]);
    let album_artist = first_string(raw_metadata, &["album_artist", "creator", "artist", "uploader"]);

    MusicMetadata {
        title,
        artist,
        album_artist,
        album,
        track_number: first_u32(raw_metadata, &["track_number", "playlist_index"]),
        disc_number: first_u32(raw_metadata, &["disc_number", "disc"]),
        genre: first_string(raw_metadata, &["genre", "genres"]),
        release_date: first_release_date(raw_metadata),
    }
}

pub fn music_metadata_from_task(metadata: &TaskMetadataInput) -> MusicMetadata {
    MusicMetadata {
        title: metadata.track_title.clone(),
        artist: metadata.artist.clone(),
        album_artist: metadata.album_artist.clone(),
        album: metadata.album.clone(),
        track_number: metadata.track_number,
        disc_number: metadata.disc_number,
        genre: metadata.genre.clone(),
        release_date: metadata.release_date.clone(),
    }
}

pub fn music_metadata_from_analysis(response: &AnalyzeOnlineMediaResponse) -> MusicMetadata {
    MusicMetadata {
        title: response.track_title.clone(),
        artist: response.artist.clone(),
        album_artist: response.album_artist.clone(),
        album: response.album.clone(),
        track_number: response.track_number,
        disc_number: response.disc_number,
        genre: response.genre.clone(),
        release_date: response.release_date.clone(),
    }
}

pub fn build_audio_metadata_pairs(
    metadata: &MusicMetadata,
    fallback_title: Option<&str>,
    fallback_artist: Option<&str>,
) -> Vec<(String, String)> {
    let mut pairs = Vec::new();

    if let Some(title) = metadata.title.as_deref().or(fallback_title).map(str::trim)
        && !title.is_empty()
    {
        pairs.push(("title".into(), title.to_string()));
    }
    if let Some(artist) = metadata.artist.as_deref().or(fallback_artist).map(str::trim)
        && !artist.is_empty()
    {
        pairs.push(("artist".into(), artist.to_string()));
    }
    if let Some(album_artist) = metadata.album_artist.as_deref().map(str::trim)
        && !album_artist.is_empty()
    {
        pairs.push(("album_artist".into(), album_artist.to_string()));
    }
    if let Some(album) = metadata.album.as_deref().map(str::trim)
        && !album.is_empty()
    {
        pairs.push(("album".into(), album.to_string()));
    }
    if let Some(track_number) = metadata.track_number {
        pairs.push(("track".into(), track_number.to_string()));
    }
    if let Some(disc_number) = metadata.disc_number {
        pairs.push(("disc".into(), disc_number.to_string()));
    }
    if let Some(genre) = metadata.genre.as_deref().map(str::trim)
        && !genre.is_empty()
    {
        pairs.push(("genre".into(), genre.to_string()));
    }
    if let Some(date) = metadata.release_date.as_deref().map(str::trim)
        && !date.is_empty()
    {
        pairs.push(("date".into(), date.to_string()));
    }

    pairs
}

fn content_type_priority(content_type: &str) -> u8 {
    match content_type {
        "adult" => 3,
        "music" => 2,
        "online_video" => 1,
        _ => 0,
    }
}

fn first_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| match value.get(key) {
        Some(Value::String(text)) => {
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        Some(Value::Array(items)) => items.iter().find_map(|item| {
            item.as_str().and_then(|text| {
                let trimmed = text.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            })
        }),
        _ => None,
    })
}

fn first_u32(value: &Value, keys: &[&str]) -> Option<u32> {
    keys.iter().find_map(|key| match value.get(key) {
        Some(Value::Number(number)) => number.as_u64().map(|value| value as u32),
        Some(Value::String(text)) => parse_u32_string(text),
        _ => None,
    })
}

fn parse_u32_string(value: &str) -> Option<u32> {
    let digits = value
        .chars()
        .skip_while(|ch| !ch.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect::<String>();

    (!digits.is_empty()).then_some(digits)?.parse::<u32>().ok()
}

fn first_release_date(value: &Value) -> Option<String> {
    ["release_date", "release_year", "upload_date"]
        .iter()
        .find_map(|key| value.get(key))
        .and_then(|entry| match entry {
            Value::String(text) => normalize_release_date(text),
            Value::Number(number) => normalize_release_date(&number.to_string()),
            _ => None,
        })
}

fn normalize_release_date(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.len() == 8 && trimmed.chars().all(|ch| ch.is_ascii_digit()) {
        return Some(format!("{}-{}-{}", &trimmed[0..4], &trimmed[4..6], &trimmed[6..8]));
    }

    if trimmed.len() == 4 && trimmed.chars().all(|ch| ch.is_ascii_digit()) {
        return Some(trimmed.to_string());
    }

    if trimmed.len() >= 10 {
        let candidate = &trimmed[0..10];
        let bytes = candidate.as_bytes();
        let is_date = bytes.get(4) == Some(&b'-')
            && bytes.get(7) == Some(&b'-')
            && candidate
                .chars()
                .enumerate()
                .all(|(index, ch)| matches!(index, 4 | 7) || ch.is_ascii_digit());
        if is_date {
            return Some(candidate.to_string());
        }
    }

    None
}

fn has_music_category(raw_metadata: Option<&Value>) -> bool {
    raw_metadata
        .and_then(|metadata| metadata.get("categories"))
        .and_then(|value| value.as_array())
        .is_some_and(|categories| {
            categories
                .iter()
                .any(|entry| entry.as_str().is_some_and(|text| text.eq_ignore_ascii_case("music")))
        })
}

fn has_music_url_hint(raw_metadata: Option<&Value>) -> bool {
    first_string(raw_metadata.unwrap_or(&Value::Null), &["webpage_url"])
        .is_some_and(|url| url.contains("music.youtube.com"))
}

#[cfg(test)]
mod tests {
    use super::{detect_ytdlp_content_type, extract_music_metadata, merge_content_types};
    use serde_json::json;

    #[test]
    fn detects_music_from_structured_metadata() {
        let metadata = json!({
            "track": "Song",
            "artist": "Artist",
            "album": "Album",
            "playlist_index": 3,
            "release_date": "20240319"
        });

        let music = extract_music_metadata(Some(&metadata));
        assert_eq!(music.title.as_deref(), Some("Song"));
        assert_eq!(music.artist.as_deref(), Some("Artist"));
        assert_eq!(music.album.as_deref(), Some("Album"));
        assert_eq!(music.track_number, Some(3));
        assert_eq!(music.release_date.as_deref(), Some("2024-03-19"));
        assert_eq!(detect_ytdlp_content_type(Some(&metadata)), "music");
    }

    #[test]
    fn does_not_detect_music_from_uploader_only() {
        // A regular Bilibili/YouTube video has `uploader` but no music-specific fields.
        // It must NOT be classified as music.
        let metadata = json!({
            "title": "2026年的Docker：发生了哪些变化？",
            "uploader": "程序猿DD",
            "creator": "程序猿DD",
            "tags": ["学习", "教程", "docker"],
        });
        assert_eq!(detect_ytdlp_content_type(Some(&metadata)), "online_video");
    }

    #[test]
    fn preserves_higher_priority_content_type() {
        assert_eq!(
            merge_content_types(Some("adult".into()), Some("online_video".into())),
            Some("adult".into())
        );
        assert_eq!(
            merge_content_types(Some("online_video".into()), Some("music".into())),
            Some("music".into())
        );
    }
}
