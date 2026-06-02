use std::env;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tokimo_package_client_api::github_releases::{GithubReleasesClient, GithubReleasesConfig};
use tracing::{info, warn};

fn first_env_var(keys: &[&str]) -> Option<std::ffi::OsString> {
    keys.iter().find_map(env::var_os)
}

fn existing_file(path: PathBuf) -> Option<PathBuf> {
    path.is_file().then_some(path)
}

fn existing_dir(path: PathBuf) -> Option<PathBuf> {
    path.is_dir().then_some(path)
}

fn candidate_workspace_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    if let Some(path) = first_env_var(&["ONLINE_MEDIA_WORKSPACE_ROOT"])
        && let Some(dir) = existing_dir(PathBuf::from(path))
    {
        roots.push(dir);
    }

    if let Ok(current_dir) = env::current_dir() {
        for ancestor in current_dir.ancestors() {
            if let Some(dir) = existing_dir(ancestor.to_path_buf())
                && !roots.iter().any(|root| root == &dir)
            {
                roots.push(dir);
            }
        }
    }

    roots
}

fn lookup_in_path(binary: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    env::split_paths(&path_var)
        .map(|entry| entry.join(binary))
        .find_map(existing_file)
}

fn lookup_in_workspace(relative_path: &str) -> Option<PathBuf> {
    candidate_workspace_roots()
        .into_iter()
        .map(|root| root.join(relative_path))
        .find_map(existing_file)
}

fn lookup_workspace_dir(relative_path: &str) -> Option<PathBuf> {
    candidate_workspace_roots()
        .into_iter()
        .map(|root| root.join(relative_path))
        .find_map(existing_dir)
}

fn ffmpeg_binary_candidates() -> Vec<&'static str> {
    if cfg!(windows) {
        vec!["ffmpeg.exe"]
    } else {
        vec!["ffmpeg"]
    }
}

fn normalize_ffmpeg_path(path: PathBuf) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path);
    }

    if path.is_dir() {
        for binary_name in ffmpeg_binary_candidates() {
            if let Some(file) = existing_file(path.join(binary_name)) {
                return Some(file);
            }
            if let Some(file) = existing_file(path.join("bin").join(binary_name)) {
                return Some(file);
            }
        }
    }

    None
}

/// Returns vendor search directories derived from `DATA_LOCAL_PATH`.
fn vendor_search_dirs() -> Vec<String> {
    if let Ok(dlp) = env::var("DATA_LOCAL_PATH") {
        vec!["bin/ffmpeg".to_string(), format!("{dlp}/vendors/ffmpeg")]
    } else {
        vec![
            "bin/ffmpeg".to_string(),
            "data/vendors/ffmpeg".to_string(),
            ".data/vendors/ffmpeg".to_string(),
        ]
    }
}

pub fn resolve_ffmpeg_binary() -> Option<PathBuf> {
    if let Some(path) = first_env_var(&["ONLINE_MEDIA_FFMPEG_BIN", "FFMPEG_BIN", "FFMPEG_LOCATION"])
        && let Some(file) = normalize_ffmpeg_path(PathBuf::from(path))
    {
        return Some(file);
    }

    for base_dir in vendor_search_dirs() {
        for binary_name in ffmpeg_binary_candidates() {
            if let Some(file) = lookup_in_workspace(&format!("{base_dir}/{binary_name}")) {
                return Some(file);
            }
        }
    }

    for binary_name in ffmpeg_binary_candidates() {
        if let Some(file) = lookup_in_path(binary_name) {
            return Some(file);
        }
    }

    for base_dir in vendor_search_dirs() {
        if let Some(dir) = lookup_workspace_dir(&base_dir) {
            return normalize_ffmpeg_path(dir);
        }
    }

    None
}

fn ytdlp_binary_candidates() -> Vec<&'static str> {
    if cfg!(windows) {
        vec!["yt-dlp.exe"]
    } else {
        vec!["yt-dlp"]
    }
}

static YTDLP_ROOT_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

pub fn set_ytdlp_root_override(root: PathBuf) -> Result<(), PathBuf> {
    YTDLP_ROOT_OVERRIDE.set(root)
}

pub fn resolve_ytdlp_binary_at(custom_root: &Path) -> Option<PathBuf> {
    for binary_name in ytdlp_binary_candidates() {
        if let Some(file) = existing_file(custom_root.join(binary_name)) {
            return Some(file);
        }
        if let Some(file) = existing_file(custom_root.join("current").join("bin").join(binary_name)) {
            return Some(file);
        }
    }

    None
}

/// Resolve yt-dlp binary. Searches process override first when set, then
/// `bin/yt-dlp/current/bin/` (deps.toml-managed), legacy flat `bin/yt-dlp/`,
/// then `{DATA_LOCAL_PATH}/vendors/yt-dlp/`.
pub fn resolve_ytdlp_binary() -> Option<PathBuf> {
    if let Some(root) = YTDLP_ROOT_OVERRIDE.get() {
        return resolve_ytdlp_binary_at(root);
    }

    let vendor_dirs = if let Ok(dlp) = env::var("DATA_LOCAL_PATH") {
        vec![
            "bin/yt-dlp/current/bin".to_string(),
            "bin/yt-dlp".to_string(),
            format!("{dlp}/vendors/yt-dlp"),
        ]
    } else {
        vec![
            "bin/yt-dlp/current/bin".to_string(),
            "bin/yt-dlp".to_string(),
            "data/vendors/yt-dlp".to_string(),
            ".data/vendors/yt-dlp".to_string(),
        ]
    };
    for binary_name in ytdlp_binary_candidates() {
        for dir in &vendor_dirs {
            if let Some(file) = lookup_in_workspace(&format!("{dir}/{binary_name}")) {
                return Some(file);
            }
        }
    }

    // Fall back to system PATH
    for binary_name in ytdlp_binary_candidates() {
        if let Some(file) = lookup_in_path(binary_name) {
            return Some(file);
        }
    }

    None
}

pub fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

// ---------------------------------------------------------------------------
// yt-dlp version / install / update
// ---------------------------------------------------------------------------

/// Get the install directory for yt-dlp: `bin/yt-dlp/` under the workspace root.
/// Consistent with ffmpeg which lives in `bin/ffmpeg/`.
pub fn ytdlp_install_dir() -> Option<PathBuf> {
    candidate_workspace_roots()
        .into_iter()
        .map(|root| root.join("bin/yt-dlp"))
        .next()
}

async fn ytdlp_version_for_binary(bin: PathBuf) -> Option<String> {
    let output = tokio::process::Command::new(&bin)
        .arg("--version")
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if version.is_empty() { None } else { Some(version) }
}

/// Run `yt-dlp --version` from a custom install root and return the version string.
pub async fn ytdlp_version_at(custom_root: &Path) -> Option<String> {
    let bin = resolve_ytdlp_binary_at(custom_root)?;
    ytdlp_version_for_binary(bin).await
}

/// Run `yt-dlp --version` and return the version string (e.g. `"2026.03.17"`).
pub async fn ytdlp_version() -> Option<String> {
    let bin = resolve_ytdlp_binary()?;
    ytdlp_version_for_binary(bin).await
}

const GITHUB_REPO: &str = "yt-dlp/yt-dlp";

/// Map current platform to the yt-dlp release asset name.
fn ytdlp_release_asset_name() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("yt-dlp_linux"),
        ("linux", "aarch64") => Some("yt-dlp_linux_aarch64"),
        ("macos", _) => Some("yt-dlp_macos"),
        ("windows", _) => Some("yt-dlp.exe"),
        _ => None,
    }
}

/// Fetch the latest release tag from GitHub (e.g. `"2026.03.17"`).
pub async fn ytdlp_latest_version() -> Result<String, String> {
    let client = GithubReleasesClient::new(GithubReleasesConfig {
        http_client: reqwest::Client::new(),
        user_agent: Some("nex-media/1.0".into()),
    });
    let release = client
        .get_latest_release(GITHUB_REPO)
        .await
        .map_err(|e| e.to_string())?;
    Ok(release.tag_name)
}

async fn ytdlp_download_to(tag: &str, install_dir: &Path) -> Result<String, String> {
    let asset = ytdlp_release_asset_name().ok_or_else(|| {
        format!(
            "unsupported platform: {} {}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;

    tokio::fs::create_dir_all(install_dir)
        .await
        .map_err(|e| format!("failed to create dir {}: {e}", install_dir.display()))?;

    let binary_name = if cfg!(windows) { "yt-dlp.exe" } else { "yt-dlp" };
    let dest = install_dir.join(binary_name);
    let temp = install_dir.join(format!(".{binary_name}.tmp"));

    let url = format!("https://github.com/{GITHUB_REPO}/releases/download/{tag}/{asset}");
    info!(url = %url, dest = %dest.display(), "downloading yt-dlp");

    let client = GithubReleasesClient::new(GithubReleasesConfig {
        http_client: reqwest::Client::new(),
        user_agent: Some("nex-media/1.0".into()),
    });
    let bytes = client
        .download_release_asset(GITHUB_REPO, tag, asset)
        .await
        .map_err(|e| e.to_string())?;

    // Write to temp file first, then rename (atomic-ish)
    tokio::fs::write(&temp, &bytes)
        .await
        .map_err(|e| format!("failed to write {}: {e}", temp.display()))?;

    // Set executable permission on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o755))
            .await
            .map_err(|e| format!("failed to chmod: {e}"))?;
    }

    tokio::fs::rename(&temp, &dest)
        .await
        .map_err(|e| format!("failed to rename temp file: {e}"))?;

    info!(version = %tag, "yt-dlp installed successfully");
    Ok(tag.to_string())
}

pub async fn ytdlp_download_at(tag: &str, custom_root: &Path) -> Result<String, String> {
    ytdlp_download_to(tag, custom_root).await
}

/// Download yt-dlp binary from GitHub releases to the legacy workspace `bin/yt-dlp/`.
/// Returns the installed version string.
pub async fn ytdlp_download(tag: &str) -> Result<String, String> {
    let install_dir = ytdlp_install_dir().ok_or("cannot determine workspace root for yt-dlp install")?;
    ytdlp_download_to(tag, &install_dir).await
}

pub async fn ensure_ytdlp_available_at(custom_root: &Path) {
    if resolve_ytdlp_binary_at(custom_root).is_some() {
        if let Some(ver) = ytdlp_version_at(custom_root).await {
            info!(version = %ver, root = %custom_root.display(), "yt-dlp binary found");
        }
        return;
    }

    info!(root = %custom_root.display(), "yt-dlp not found, downloading latest release...");
    match ytdlp_latest_version().await {
        Ok(tag) => match ytdlp_download_at(&tag, custom_root).await {
            Ok(ver) => info!(version = %ver, root = %custom_root.display(), "yt-dlp auto-installed"),
            Err(e) => warn!(root = %custom_root.display(), "failed to auto-install yt-dlp: {e}"),
        },
        Err(e) => warn!(root = %custom_root.display(), "failed to check latest yt-dlp version: {e}"),
    }
}

/// Ensure yt-dlp is available, downloading it if missing.
/// Called on server startup. Non-fatal: logs a warning on failure.
pub async fn ensure_ytdlp_available() {
    if resolve_ytdlp_binary().is_some() {
        if let Some(ver) = ytdlp_version().await {
            info!(version = %ver, "yt-dlp binary found");
        }
        return;
    }

    info!("yt-dlp not found, downloading latest release...");
    match ytdlp_latest_version().await {
        Ok(tag) => match ytdlp_download(&tag).await {
            Ok(ver) => info!(version = %ver, "yt-dlp auto-installed"),
            Err(e) => warn!("failed to auto-install yt-dlp: {e}"),
        },
        Err(e) => warn!("failed to check latest yt-dlp version: {e}"),
    }
}
