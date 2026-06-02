pub mod engine;
pub mod models;
pub mod music;
pub mod native;
pub mod provider_catalog;
pub mod providers;
pub mod router;
pub mod runtime;
pub mod subtitles;
pub mod task_manager;
pub mod thumbnail;
pub mod tooling;

use std::path::PathBuf;
use std::sync::Arc;

use task_manager::TaskManager;

#[derive(Clone)]
pub struct AppState {
    pub staging_root: PathBuf,
    pub tasks: Arc<TaskManager>,
}

pub use router::build_app;
