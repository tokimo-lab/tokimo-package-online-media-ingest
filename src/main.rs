use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tokimo_package_online_media_ingest::{AppState, build_app};
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::EnvFilter;

use tokimo_package_online_media_ingest::task_manager::TaskManager;

#[derive(Parser, Debug)]
#[command(name = "rust-online-media-ingest")]
#[command(about = "Online media ingest server for nex-media")]
struct Args {
    #[arg(long, default_value = "0.0.0.0:4090")]
    listen: String,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let args = Args::parse();
    let data_local_path = std::env::var("DATA_LOCAL_PATH").unwrap_or_else(|_| "./.data".to_string());
    let state = Arc::new(AppState {
        staging_root: PathBuf::from(format!("{data_local_path}/online-media-ingest")),
        tasks: Arc::new(TaskManager::new()),
    });

    // Auto-install yt-dlp if missing (non-blocking background task)
    tokio::spawn(tokimo_package_online_media_ingest::tooling::ensure_ytdlp_available());

    let app = build_app(state);
    let listener = TcpListener::bind(&args.listen).await.unwrap();
    info!(listen = %args.listen, "rust-online-media-ingest server listening");
    axum::serve(listener, app).await.unwrap();
}
