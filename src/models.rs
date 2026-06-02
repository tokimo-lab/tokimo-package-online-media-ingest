use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnlineMediaProvider {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub supported_content_types: Vec<String>,
    pub requires_auth: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnlineMediaCapability {
    pub can_analyze: bool,
    pub can_download: bool,
    pub can_import_metadata: bool,
    pub supports_collections: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzeOnlineMediaRequest {
    pub url: String,
    pub target_library_id: Option<String>,
    pub preferred_provider: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzeOnlineMediaResponse {
    pub is_supported: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<OnlineMediaProvider>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability: Option<OnlineMediaCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_site: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normalized_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uploader: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_number: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disc_number: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    pub requires_auth: bool,
    pub warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_metadata: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleSearchRequest {
    pub query: Option<String>,
    pub imdb_id: Option<String>,
    pub tmdb_id: Option<String>,
    pub languages: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleSearchResult {
    pub id: String,
    pub name: String,
    pub language: String,
    pub language_name: String,
    pub format: String,
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub movie_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_group: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleDownloadRequest {
    pub subtitle_id: String,
    pub detail_path: Option<String>,
    pub download_path: Option<String>,
    pub language: String,
    pub format: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleDownloadResponse {
    pub name: String,
    pub format: String,
    pub content_base64: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTaskRequest {
    pub record_id: String,
    pub url: String,
    pub normalized_url: Option<String>,
    pub provider_id: Option<String>,
    pub auth: Option<TaskAuthInput>,
    pub audio_only: Option<bool>,
    pub audio_container: Option<String>,
    pub target_library_id: String,
    pub target_folder_config_snapshot: TargetFolderConfigSnapshot,
    pub metadata: TaskMetadataInput,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskAuthInput {
    pub cookie_header: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetFolderConfigSnapshot {
    pub id: String,
    pub content_type: String,
    pub download_path: String,
    pub target_path: String,
    pub link_mode: String,
    pub organize_lang: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskMetadataInput {
    pub title: Option<String>,
    pub media_title: Option<String>,
    pub media_year: Option<String>,
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
    pub source_id: Option<String>,
    pub external_id: Option<String>,
    pub source_site: Option<String>,
    pub generate_nfo: Option<bool>,
    pub raw_metadata: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTaskResponse {
    pub task_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputFile {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub source_url: String,
    pub provider_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_site: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uploader: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_number: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disc_number: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
    pub output_files: Vec<OutputFile>,
    pub artifacts: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_metadata: Option<Value>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatusResponse {
    pub task_id: String,
    pub status: TaskState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downloaded_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eta_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_path: Option<String>,
    pub output_files: Vec<OutputFile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CancelTaskResponse {
    pub success: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthResponse {
    pub ok: bool,
}

// --- Batch / Playlist models ---

/// Request to resolve a collection (playlist, channel, series) into individual items.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveCollectionRequest {
    pub url: String,
    pub provider_id: Option<String>,
    pub auth: Option<TaskAuthInput>,
    /// Maximum number of items to return. `None` = all.
    pub limit: Option<usize>,
    /// Pagination cursor from a previous response (provider-specific opaque string).
    pub cursor: Option<String>,
}

/// A single item within a resolved collection.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionItem {
    pub url: String,
    pub title: Option<String>,
    pub thumbnail_url: Option<String>,
    pub duration_seconds: Option<u64>,
    pub index: usize,
    pub source_id: Option<String>,
}

/// Response containing the resolved items of a collection.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveCollectionResponse {
    pub provider_id: String,
    pub collection_title: Option<String>,
    pub collection_url: String,
    pub total_items: Option<usize>,
    pub items: Vec<CollectionItem>,
    /// Cursor for fetching the next page, if more items exist.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Request to create download tasks for multiple items from a collection.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchCreateTasksRequest {
    pub items: Vec<BatchTaskItem>,
    pub target_library_id: String,
    pub target_folder_config_snapshot: TargetFolderConfigSnapshot,
    pub auth: Option<TaskAuthInput>,
}

/// A single item in a batch create request.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchTaskItem {
    pub url: String,
    pub provider_id: Option<String>,
    pub metadata: TaskMetadataInput,
    pub record_id: String,
    pub audio_only: Option<bool>,
    pub audio_container: Option<String>,
}

/// Response for batch task creation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchCreateTasksResponse {
    pub task_ids: Vec<String>,
    pub total: usize,
}

/// A single provider entry for the supported-sites list.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderListEntry {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub source_site: String,
    pub supported_content_types: Vec<String>,
    pub requires_auth: bool,
    pub auth_configurable: bool,
    pub common_source_sites: Vec<String>,
    pub source_site_aliases: Vec<String>,
    pub host_suffixes: Vec<String>,
}

/// Response containing all supported providers.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderListResponse {
    pub providers: Vec<ProviderListEntry>,
    pub ytdlp_available: bool,
}

/// yt-dlp installation status.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YtdlpStatusResponse {
    pub installed: bool,
    pub current_version: Option<String>,
    pub latest_version: Option<String>,
    pub needs_update: bool,
}

/// Result of a yt-dlp install/update operation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YtdlpUpdateResponse {
    pub success: bool,
    pub version: Option<String>,
    pub message: String,
}
