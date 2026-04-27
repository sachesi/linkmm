//! UMU-launcher integration.
//!
//! Manages the `umu-run` binary used to launch non-Steam Windows games through
//! Proton without requiring a Steam installation.
//!
//! ## Storage layout
//!
//! ```text
//! ~/.local/share/linkmm/
//!   umu-run          ← the extracted umu-run binary (chmod +x)
//! ```
//!
//! ## How `umu-run` is obtained
//!
//! 1. Query `https://api.github.com/repos/Open-Wine-Components/umu-launcher/releases/latest`
//!    and parse the `tag_name` field (e.g. `"1.4.0"`).
//! 2. Build the zipapp tarball URL:
//!    `https://github.com/Open-Wine-Components/umu-launcher/releases/download/{tag}/umu-launcher-{tag}-zipapp.tar`
//! 3. Download the `.tar` archive.
//! 4. Extract `umu/umu-run` (or any path ending in `umu-run`) from the archive.
//! 5. Write it to `~/.local/share/linkmm/umu-run` and `chmod 755`.
//!
//! ## Version tracking
//!
//! The installed tag is persisted in `AppConfig::umu_installed_version`
//! (saved in `config.toml`).  On every app startup, [`check_and_update`]
//! queries GitHub in a background thread and re-downloads when the latest tag
//! differs from the stored one.
//!
//! ## Minimum launch environment required by umu-run
//!
//! | Variable      | Value                                                        |
//! |---------------|--------------------------------------------------------------|
//! | `WINEPREFIX`  | per-game prefix, default `~/.local/share/umu/default`       |
//! | `GAMEID`      | `"umu-<steam_app_id>"` — enables protonfixes for the game   |
//! | `PROTONPATH`  | `"GE-Proton"` (auto-download) or explicit path              |
//! | `STORE`       | `"none"`                                                     |
//! | `UMU_LOG`     | `"1"`                                                        |

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::rc::Rc;
use crate::core::config::AppConfig;
use crate::core::steam::split_launch_arguments;

// ── Constants ─────────────────────────────────────────────────────────────────

/// GitHub releases API – latest umu-launcher release.
const GITHUB_API_LATEST: &str =
    "https://api.github.com/repos/Open-Wine-Components/umu-launcher/releases/latest";

/// Template for the zipapp tarball URL.  `{tag}` is substituted at runtime.
const ZIPAPP_URL_TEMPLATE: &str = "https://github.com/Open-Wine-Components/umu-launcher/releases/download\
     /{tag}/umu-launcher-{tag}-zipapp.tar";

/// Default Wine prefix used when none is configured.
const DEFAULT_WINEPREFIX_SUBDIR: &str = "umu/default";

// ── Path helpers ──────────────────────────────────────────────────────────────

/// `~/.local/share/linkmm/umu-run`
pub fn umu_run_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("linkmm")
        .join("umu-run")
}

/// `~/.local/share/umu/default`
///
/// This is the default `WINEPREFIX` used when none is configured.
pub fn default_wineprefix() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(DEFAULT_WINEPREFIX_SUBDIR)
}

// ── Availability ──────────────────────────────────────────────────────────────

/// Return `true` when `umu-run` is present and executable.
pub fn is_umu_available() -> bool {
    let path = umu_run_path();
    if !path.exists() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(&path)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    true
}

// ── GitHub API ────────────────────────────────────────────────────────────────

/// Query the GitHub API and return the latest release tag (e.g. `"1.4.0"`).
pub fn fetch_latest_tag() -> Result<String, String> {
    let response = ureq::get(GITHUB_API_LATEST)
        .set("User-Agent", "Linkmm/0.1.0")
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("GitHub API request failed: {e}"))?;

    let body: serde_json::Value = response
        .into_json()
        .map_err(|e| format!("Failed to parse GitHub API response: {e}"))?;

    body.get("tag_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "GitHub API response missing 'tag_name' field".to_string())
}

/// Build the zipapp tarball download URL for a given `tag`.
fn zipapp_url(tag: &str) -> String {
    ZIPAPP_URL_TEMPLATE.replace("{tag}", tag)
}

// ── Download + extraction ─────────────────────────────────────────────────────

/// Download the umu-launcher zipapp tarball for `tag`, extract `umu-run` from
/// it, write the binary to [`umu_run_path()`], and `chmod 755` it.
pub fn download_and_install(
    tag: &str,
    mut on_progress: impl FnMut(u64, u64) -> bool,
) -> Result<PathBuf, String> {
    let url = zipapp_url(tag);
    log::info!("Downloading umu-launcher {tag} from {url}");

    // ── 1. Ensure parent directory exists ────────────────────────────────
    let run_path = umu_run_path();
    if let Some(parent) = run_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create linkmm data directory: {e}"))?;
    }

    // ── 2. Download the tarball into memory ──────────────────────────────
    let tar_bytes = download_to_memory(&url, &mut on_progress)?;
    log::info!("Downloaded {} bytes, extracting umu-run…", tar_bytes.len());

    // ── 3. Extract umu-run from the archive ──────────────────────────────
    let umu_run_bytes = extract_umu_run_from_tar(&tar_bytes)?;
    log::info!("Extracted umu-run ({} bytes)", umu_run_bytes.len());

    // ── 4. Write to disk ─────────────────────────────────────────────────
    std::fs::write(&run_path, &umu_run_bytes)
        .map_err(|e| format!("Failed to write umu-run to {}: {e}", run_path.display()))?;

    // ── 5. chmod +x ──────────────────────────────────────────────────────
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&run_path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("Failed to set executable bit on umu-run: {e}"))?;
    }

    log::info!("umu-run {tag} installed at {}", run_path.display());
    Ok(run_path)
}

/// Download `url` into a `Vec<u8>`, reporting progress.
fn download_to_memory(
    url: &str,
    mut on_progress: impl FnMut(u64, u64) -> bool,
) -> Result<Vec<u8>, String> {
    let response = ureq::get(url)
        .set("User-Agent", "Linkmm/0.1.0")
        .call()
        .map_err(|e| format!("Download request failed: {e}"))?;

    let total: u64 = response
        .header("Content-Length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let mut buf = Vec::with_capacity(total as usize);
    let mut reader = response.into_reader();
    let mut chunk = [0u8; 65536];
    let mut downloaded: u64 = 0;

    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                downloaded += n as u64;
                if !on_progress(downloaded, total) {
                    return Err("Download cancelled".to_string());
                }
            }
            Err(e) => return Err(format!("Read error during download: {e}")),
        }
    }

    Ok(buf)
}

/// Walk the tar archive in `tar_bytes` and return the raw content of the first
/// entry whose path ends with `umu-run`.
fn extract_umu_run_from_tar(tar_bytes: &[u8]) -> Result<Vec<u8>, String> {
    let cursor = std::io::Cursor::new(tar_bytes);
    let mut archive = tar::Archive::new(cursor);

    for entry in archive
        .entries()
        .map_err(|e| format!("Failed to read tar entries: {e}"))?
    {
        let mut entry = entry.map_err(|e| format!("Corrupt tar entry: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("Invalid tar entry path: {e}"))?
            .to_path_buf();

        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if file_name == "umu-run" {
            log::debug!("Found umu-run in archive at: {}", path.display());
            let mut data = Vec::new();
            entry
                .read_to_end(&mut data)
                .map_err(|e| format!("Failed to read umu-run from tar: {e}"))?;
            return Ok(data);
        }
    }

    Err("umu-run binary not found in the downloaded tarball.".to_string())
}

// ── High-level install / update ───────────────────────────────────────────────

pub fn ensure_umu_available(
    installed_version: Option<&str>,
    mut on_progress: impl FnMut(u64, u64) -> bool,
) -> Result<(String, PathBuf), String> {
    let latest_tag = fetch_latest_tag()?;
    let needs_download =
        !is_umu_available() || installed_version.map(|v| v != latest_tag).unwrap_or(true);

    if !needs_download {
        return Ok((latest_tag, umu_run_path()));
    }

    let path = download_and_install(&latest_tag, &mut on_progress)?;
    Ok((latest_tag, path))
}

pub fn check_and_update_in_background(
    installed_version: Option<String>,
    on_updated: impl Fn(String) + 'static,
) {
    let on_updated = Rc::new(on_updated);
    let (tx, rx) = mpsc::channel::<Result<String, String>>();

    std::thread::spawn(move || {
        let result = ensure_umu_available(installed_version.as_deref(), |_, _| true);
        let _ = tx.send(result.map(|(tag, _)| tag));
    });

    glib::timeout_add_local(std::time::Duration::from_millis(250), move || {
        match rx.try_recv() {
            Ok(Ok(tag)) => {
                on_updated(tag);
                glib::ControlFlow::Break
            }
            Ok(Err(e)) => {
                log::warn!("Background umu update failed: {e}");
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        }
    });
}

// ── Launch ────────────────────────────────────────────────────────────────────

pub fn launch_with_umu(
    exe_path: &Path,
    arguments: &str,
    steam_app_id: u32,
    prefix_path: Option<&Path>,
    proton_path: Option<&Path>,
    store: &str,
    steam_root: Option<&Path>,
) -> Result<std::process::Child, String> {
    let mut command =
        build_umu_command(exe_path, arguments, steam_app_id, prefix_path, proton_path, store, steam_root)?;
    command
        .spawn()
        .map_err(|e| format!("Failed to spawn umu-run: {e}"))
}

pub fn build_umu_command(
    exe_path: &Path,
    arguments: &str,
    steam_app_id: u32,
    prefix_path: Option<&Path>,
    proton_path: Option<&Path>,
    store: &str,
    steam_root: Option<&Path>,
) -> Result<std::process::Command, String> {
    if !is_umu_available() {
        return Err("umu-run is not installed.".to_string());
    }

    let wineprefix = prefix_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(default_wineprefix);

    let game_id = format!("umu-{steam_app_id}");

    let proton_path_str: std::borrow::Cow<str> = match proton_path {
        Some(p) => p.to_string_lossy(),
        None => "GE-Proton".into(),
    };

    let mut command = std::process::Command::new(umu_run_path());
    command
        .current_dir(exe_path.parent().unwrap_or_else(|| Path::new("/")))
        .arg(exe_path)
        .env("WINEPREFIX", &wineprefix)
        .env("GAMEID", &game_id)
        .env("PROTONPATH", proton_path_str.as_ref())
        .env("STORE", store)
        .env("UMU_LOG", "1");
    
    let steam_root_owned = crate::core::steam::library::find_steam_root();
    if let Some(root) = steam_root.or_else(|| steam_root_owned.as_deref()) {
        command.env("STEAM_COMPAT_CLIENT_INSTALL_PATH", root);
    }

    if !arguments.trim().is_empty() {
        for arg in split_launch_arguments(arguments)? {
            command.arg(arg);
        }
    }

    Ok(command)
}
