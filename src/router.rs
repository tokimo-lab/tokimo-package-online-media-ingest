use std::sync::Arc;

use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::{get, post},
};

use crate::AppState;
use crate::models::{
    AnalyzeOnlineMediaRequest, BatchCreateTasksRequest, BatchCreateTasksResponse, CancelTaskResponse,
    CreateTaskRequest, CreateTaskResponse, HealthResponse, ProviderListResponse, ResolveCollectionRequest,
    ResolveCollectionResponse, TaskStatusResponse, YtdlpStatusResponse, YtdlpUpdateResponse,
};
use crate::providers::{analyze_url, resolve_collection_url};
use crate::runtime::spawn_task;

pub fn build_app(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health).post(health))
        .route("/api/providers", get(list_providers))
        .route("/api/analyze", post(analyze))
        .route("/api/tasks", post(create_task))
        .route("/api/tasks/{task_id}", get(get_task))
        .route("/api/tasks/{task_id}/cancel", post(cancel_task))
        .route("/api/collection/resolve", post(resolve_collection))
        .route("/api/tasks/batch", post(batch_create_tasks))
        .route("/api/ytdlp/status", get(ytdlp_status))
        .route("/api/ytdlp/update", post(ytdlp_update))
        .with_state(state)
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { ok: true })
}

async fn list_providers() -> Json<ProviderListResponse> {
    Json(crate::provider_catalog::list_all_providers_with_ytdlp().await)
}

async fn analyze(
    State(_state): State<Arc<AppState>>,
    Json(input): Json<AnalyzeOnlineMediaRequest>,
) -> Json<crate::models::AnalyzeOnlineMediaResponse> {
    Json(analyze_url(&input).await)
}

async fn create_task(
    State(state): State<Arc<AppState>>,
    Json(input): Json<CreateTaskRequest>,
) -> Json<CreateTaskResponse> {
    let task_id = state.tasks.create_task(input.clone()).await;
    spawn_task((*state).clone(), task_id.clone(), input);
    Json(CreateTaskResponse { task_id })
}

async fn get_task(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
) -> Result<Json<TaskStatusResponse>, StatusCode> {
    state
        .tasks
        .get_task(&task_id)
        .await
        .map(|task| Json(task.to_response()))
        .ok_or(StatusCode::NOT_FOUND)
}

async fn cancel_task(State(state): State<Arc<AppState>>, Path(task_id): Path<String>) -> Json<CancelTaskResponse> {
    Json(CancelTaskResponse {
        success: state.tasks.request_cancel(&task_id).await,
    })
}

async fn resolve_collection(
    State(_state): State<Arc<AppState>>,
    Json(input): Json<ResolveCollectionRequest>,
) -> Result<Json<ResolveCollectionResponse>, StatusCode> {
    resolve_collection_url(&input)
        .await
        .map(Json)
        .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)
}

async fn batch_create_tasks(
    State(state): State<Arc<AppState>>,
    Json(input): Json<BatchCreateTasksRequest>,
) -> Json<BatchCreateTasksResponse> {
    let mut task_ids = Vec::with_capacity(input.items.len());

    for item in &input.items {
        let request = CreateTaskRequest {
            record_id: item.record_id.clone(),
            url: item.url.clone(),
            normalized_url: None,
            provider_id: item.provider_id.clone(),
            auth: input.auth.clone(),
            audio_only: item.audio_only,
            audio_container: item.audio_container.clone(),
            target_library_id: input.target_library_id.clone(),
            target_folder_config_snapshot: input.target_folder_config_snapshot.clone(),
            metadata: item.metadata.clone(),
        };
        let task_id = state.tasks.create_task(request.clone()).await;
        spawn_task((*state).clone(), task_id.clone(), request);
        task_ids.push(task_id);
    }

    let total = task_ids.len();
    Json(BatchCreateTasksResponse { task_ids, total })
}

async fn ytdlp_status() -> Json<YtdlpStatusResponse> {
    let installed = crate::tooling::resolve_ytdlp_binary().is_some();
    let current_version = if installed {
        crate::tooling::ytdlp_version().await
    } else {
        None
    };
    let latest_version = crate::tooling::ytdlp_latest_version().await.ok();
    let needs_update = match (&current_version, &latest_version) {
        (Some(cur), Some(lat)) => cur != lat,
        _ => false,
    };
    Json(YtdlpStatusResponse {
        installed,
        current_version,
        latest_version,
        needs_update,
    })
}

async fn ytdlp_update() -> Json<YtdlpUpdateResponse> {
    let tag = match crate::tooling::ytdlp_latest_version().await {
        Ok(t) => t,
        Err(e) => {
            return Json(YtdlpUpdateResponse {
                success: false,
                version: None,
                message: e,
            });
        }
    };
    match crate::tooling::ytdlp_download(&tag).await {
        Ok(ver) => Json(YtdlpUpdateResponse {
            success: true,
            version: Some(ver),
            message: "ok".into(),
        }),
        Err(e) => Json(YtdlpUpdateResponse {
            success: false,
            version: None,
            message: e,
        }),
    }
}
