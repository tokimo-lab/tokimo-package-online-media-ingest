use std::collections::HashMap;

use tokio::sync::RwLock;
use uuid::Uuid;

use crate::models::{CreateTaskRequest, OutputFile, TaskState, TaskStatusResponse};

#[derive(Debug, Clone)]
pub struct TaskRecord {
    pub task_id: String,
    pub request: CreateTaskRequest,
    pub status: TaskState,
    pub stage: Option<String>,
    pub progress: Option<f64>,
    pub downloaded_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    pub speed_bytes: Option<u64>,
    pub eta_seconds: Option<u64>,
    pub manifest_path: Option<String>,
    pub target_path: Option<String>,
    pub output_files: Vec<OutputFile>,
    pub error: Option<String>,
    pub cancel_requested: bool,
}

impl TaskRecord {
    pub fn to_response(&self) -> TaskStatusResponse {
        TaskStatusResponse {
            task_id: self.task_id.clone(),
            status: self.status,
            stage: self.stage.clone(),
            progress: self.progress,
            downloaded_bytes: self.downloaded_bytes,
            total_bytes: self.total_bytes,
            speed_bytes: self.speed_bytes,
            eta_seconds: self.eta_seconds,
            manifest_path: self.manifest_path.clone(),
            target_path: self.target_path.clone(),
            output_files: self.output_files.clone(),
            error: self.error.clone(),
        }
    }
}

pub struct TaskManager {
    tasks: RwLock<HashMap<String, TaskRecord>>,
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
        }
    }

    pub async fn create_task(&self, request: CreateTaskRequest) -> String {
        let task_id = Uuid::new_v4().to_string();
        let record = TaskRecord {
            task_id: task_id.clone(),
            request,
            status: TaskState::Pending,
            stage: Some("queued".into()),
            progress: Some(0.0),
            downloaded_bytes: None,
            total_bytes: None,
            speed_bytes: None,
            eta_seconds: None,
            manifest_path: None,
            target_path: None,
            output_files: Vec::new(),
            error: None,
            cancel_requested: false,
        };

        self.tasks.write().await.insert(task_id.clone(), record);
        task_id
    }

    pub async fn get_task(&self, task_id: &str) -> Option<TaskRecord> {
        self.tasks.read().await.get(task_id).cloned()
    }

    pub async fn is_cancel_requested(&self, task_id: &str) -> bool {
        self.tasks
            .read()
            .await
            .get(task_id)
            .is_some_and(|task| task.cancel_requested)
    }

    pub async fn request_cancel(&self, task_id: &str) -> bool {
        let mut tasks = self.tasks.write().await;
        let Some(task) = tasks.get_mut(task_id) else {
            return false;
        };

        task.cancel_requested = true;
        if matches!(task.status, TaskState::Pending | TaskState::Running) {
            task.status = TaskState::Cancelled;
            task.stage = Some("cancelled".into());
            task.error = Some("task cancelled".into());
            task.progress = Some(task.progress.unwrap_or(0.0));
        }
        true
    }

    pub async fn update_stage(&self, task_id: &str, stage: &str, progress: f64) {
        if let Some(task) = self.tasks.write().await.get_mut(task_id) {
            if task.cancel_requested && task.status != TaskState::Cancelled {
                task.status = TaskState::Cancelled;
                task.stage = Some("cancelled".into());
                task.error = Some("task cancelled".into());
                return;
            }

            task.status = TaskState::Running;
            task.stage = Some(stage.to_string());
            task.progress = Some(progress.clamp(0.0, 100.0));
        }
    }

    pub async fn update_download_progress(
        &self,
        task_id: &str,
        progress: Option<f64>,
        downloaded_bytes: Option<u64>,
        total_bytes: Option<u64>,
        speed_bytes: Option<u64>,
        eta_seconds: Option<u64>,
    ) {
        if let Some(task) = self.tasks.write().await.get_mut(task_id) {
            if task.cancel_requested && task.status != TaskState::Cancelled {
                task.status = TaskState::Cancelled;
                task.stage = Some("cancelled".into());
                task.error = Some("task cancelled".into());
                return;
            }

            task.status = TaskState::Running;
            task.stage = Some("downloading".into());
            if let Some(progress) = progress {
                task.progress = Some(progress.clamp(0.0, 100.0));
            } else if let (Some(downloaded), Some(total)) = (downloaded_bytes, total_bytes)
                && total > 0
            {
                task.progress =
                    Some(((downloaded as f64 / total as f64) * 100.0).clamp(0.0, 100.0));
            }
            task.downloaded_bytes = downloaded_bytes.or(task.downloaded_bytes);
            task.total_bytes = total_bytes.or(task.total_bytes);
            task.speed_bytes = speed_bytes;
            task.eta_seconds = eta_seconds;
        }
    }

    pub async fn complete(
        &self,
        task_id: &str,
        manifest_path: Option<String>,
        target_path: Option<String>,
        output_files: Vec<OutputFile>,
    ) {
        if let Some(task) = self.tasks.write().await.get_mut(task_id) {
            if task.cancel_requested {
                task.status = TaskState::Cancelled;
                task.stage = Some("cancelled".into());
                task.error = Some("task cancelled".into());
                return;
            }

            task.status = TaskState::Completed;
            task.stage = Some("completed".into());
            task.progress = Some(100.0);
            task.downloaded_bytes = output_files.iter().filter_map(|file| file.size_bytes).max();
            task.total_bytes = task.downloaded_bytes;
            task.speed_bytes = None;
            task.eta_seconds = Some(0);
            task.manifest_path = manifest_path;
            task.target_path = target_path;
            task.output_files = output_files;
            task.error = None;
        }
    }

    pub async fn fail(&self, task_id: &str, message: String) {
        if let Some(task) = self.tasks.write().await.get_mut(task_id) {
            if task.cancel_requested {
                task.status = TaskState::Cancelled;
                task.stage = Some("cancelled".into());
                task.error = Some("task cancelled".into());
                return;
            }

            task.status = TaskState::Failed;
            task.stage = Some("failed".into());
            task.error = Some(message);
        }
    }
}
