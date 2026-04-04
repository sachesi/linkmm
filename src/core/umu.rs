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

use std::io::Read;
use std::path::{Path, PathBuf};
#[cfg(feature = "ui")]
use std::rc::Rc;
#[cfg(feature = "ui")]
use std::sync::mpsc;

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
        return std::fs::metadata(&path)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false);
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
///
/// `on_progress` is called with `(bytes_downloaded, total_bytes)`.
/// Return `true` to continue, `false` to cancel.
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
/// entry whose path ends with `umu-run` (without a `.py` extension).
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

        // Match any entry whose final component is exactly "umu-run"
        // (not "umu-run.py" or similar).
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

    Err("umu-run binary not found in the downloaded tarball. \
         The archive layout may have changed — please report this."
        .to_string())
}

// ── High-level install / update ───────────────────────────────────────────────

/// Ensure `umu-run` is present and up-to-date.
///
/// - Fetches the latest GitHub release tag.
/// - Compares with `installed_version` (the tag stored in `AppConfig`).
/// - Downloads and installs when missing or outdated.
/// - Returns `(tag, path)` on success.
///
/// `on_progress` receives `(bytes_downloaded, total_bytes)`.
pub fn ensure_umu_available(
    installed_version: Option<&str>,
    mut on_progress: impl FnMut(u64, u64) -> bool,
) -> Result<(String, PathBuf), String> {
    // ── 1. Fetch the latest tag ──────────────────────────────────────────
    let latest_tag = fetch_latest_tag()?;
    log::info!("Latest umu-launcher release: {latest_tag}");

    // ── 2. Decide whether we need to (re-)download ───────────────────────
    let needs_download =
        !is_umu_available() || installed_version.map(|v| v != latest_tag).unwrap_or(true);

    if !needs_download {
        log::info!(
            "umu-run {latest_tag} is already up-to-date at {}",
            umu_run_path().display()
        );
        return Ok((latest_tag, umu_run_path()));
    }

    // ── 3. Download and install ──────────────────────────────────────────
    let path = download_and_install(&latest_tag, &mut on_progress)?;
    Ok((latest_tag, path))
}

// ── Startup background update check ──────────────────────────────────────────

/// Check for a newer umu-launcher release in a background thread and
/// re-download if the latest tag differs from `installed_version`.
///
/// The `on_updated` callback is invoked on the **GTK main thread** (via
/// `glib::idle_add_local`) with the new tag string when an update was
/// installed.  Errors are logged but not surfaced to the user (the existing
/// binary remains usable).
///
/// Call this once from `build_ui` after the main window is shown.
#[cfg(feature = "ui")]
pub fn check_and_update_in_background(
    installed_version: Option<String>,
    on_updated: impl Fn(String) + 'static,
) {
    let on_updated = Rc::new(on_updated);

    let (tx, rx) = mpsc::channel::<Result<String, String>>();

    std::thread::spawn(move || {
        let result = ensure_umu_available(
            installed_version.as_deref(),
            // progress callback — not shown for background updates
            |_, _| true,
        );
        let _ = tx.send(result.map(|(tag, _)| tag));
    });

    // Poll the channel on the GTK main-loop until we get a result.
    glib::timeout_add_local(std::time::Duration::from_millis(250), move || {
        match rx.try_recv() {
            Ok(Ok(tag)) => {
                log::info!("umu-launcher update complete: {tag}");
                on_updated(tag);
                glib::ControlFlow::Break
            }
            Ok(Err(e)) => {
                // Non-fatal: existing binary (if any) is still usable.
                log::warn!("umu-launcher background update failed: {e}");
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => {
                log::warn!("umu-launcher update thread disconnected unexpectedly");
                glib::ControlFlow::Break
            }
        }
    });
}

#[cfg(not(feature = "ui"))]
pub fn check_and_update_in_background(
    _installed_version: Option<String>,
    _on_updated: impl Fn(String) + 'static,
) {
}

// ── Launch ────────────────────────────────────────────────────────────────────

/// Launch a game executable through `umu-run`.
///
/// | Env var       | Value                                                   |
/// |---------------|---------------------------------------------------------|
/// | `WINEPREFIX`  | `prefix_path` or [`default_wineprefix()`]               |
/// | `GAMEID`      | `"umu-<steam_app_id>"`                                  |
/// | `PROTONPATH`  | `"GE-Proton"` (auto-download GE-Proton) or custom path |
/// | `STORE`       | `"none"`                                                |
/// | `UMU_LOG`     | `"1"`                                                   |
pub fn launch_with_umu(
    exe_path: &Path,
    steam_app_id: u32,
    prefix_path: Option<&Path>,
    proton_path: Option<&Path>,
) -> Result<std::process::Child, String> {
    if !is_umu_available() {
        return Err(
            "umu-run is not installed. Configure a non-Steam game first to trigger the download."
                .to_string(),
        );
    }

    if !exe_path.exists() {
        return Err(format!("Game executable not found: {}", exe_path.display()));
    }

    let wineprefix = prefix_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(default_wineprefix);

    let game_id = format!("umu-{steam_app_id}");

    let proton_path_str: std::borrow::Cow<str> = match proton_path {
        Some(p) => p.to_string_lossy(),
        None => "GE-Proton".into(),
    };

    log::info!("Launching via umu-run: {}", exe_path.display());
    log::debug!("  WINEPREFIX = {}", wineprefix.display());
    log::debug!("  GAMEID     = {game_id}");
    log::debug!("  PROTONPATH = {proton_path_str}");
    log::debug!("  STORE      = none");
    log::debug!("  UMU_LOG    = 1");

    std::process::Command::new(umu_run_path())
        .arg(exe_path)
        .env("WINEPREFIX", &wineprefix)
        .env("GAMEID", &game_id)
        .env("PROTONPATH", proton_path_str.as_ref())
        .env("STORE", "none")
        .env("UMU_LOG", "1")
        .spawn()
        .map_err(|e| format!("Failed to spawn umu-run: {e}"))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn umu_run_path_ends_with_linkmm_umu_run() {
        let p = umu_run_path();
        assert!(
            p.ends_with("linkmm/umu-run"),
            "unexpected path: {}",
            p.display()
        );
    }

    #[test]
    fn default_wineprefix_ends_with_umu_default() {
        let p = default_wineprefix();
        assert!(
            p.ends_with("umu/default"),
            "unexpected prefix: {}",
            p.display()
        );
    }

    #[test]
    fn is_umu_available_false_when_file_missing() {
        if !umu_run_path().exists() {
            assert!(!is_umu_available());
        }
    }

    #[test]
    fn zipapp_url_contains_tag_twice() {
        let url = zipapp_url("1.4.0");
        assert!(
            url.contains("/1.4.0/"),
            "URL should contain tag in path: {url}"
        );
        assert!(
            url.contains("umu-launcher-1.4.0-zipapp.tar"),
            "URL should contain versioned filename: {url}"
        );
    }

    #[test]
    fn extract_umu_run_finds_entry_named_umu_run() {
        // Build a minimal in-memory tar that contains one entry named "umu/umu-run"
        // with known content, then verify extraction succeeds.
        let mut tar_builder = tar::Builder::new(Vec::new());
        let content = b"#!/usr/bin/env python3\n# fake umu-run\n";
        let mut header = tar::Header::new_gnu();
        header.set_path("umu/umu-run").unwrap();
        header.set_size(content.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar_builder.append(&header, &content[..]).unwrap();
        let tar_bytes = tar_builder.into_inner().unwrap();

        let extracted =
            extract_umu_run_from_tar(&tar_bytes).expect("should find umu-run in the archive");
        assert_eq!(extracted, content);
    }

    #[test]
    fn extract_umu_run_errors_when_entry_absent() {
        // A tar with no entry named "umu-run" should produce a descriptive error.
        let mut tar_builder = tar::Builder::new(Vec::new());
        let content = b"unrelated";
        let mut header = tar::Header::new_gnu();
        header.set_path("other/file.txt").unwrap();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar_builder.append(&header, &content[..]).unwrap();
        let tar_bytes = tar_builder.into_inner().unwrap();

        let result = extract_umu_run_from_tar(&tar_bytes);
        assert!(result.is_err(), "expected error when umu-run is absent");
        assert!(
            result.unwrap_err().contains("umu-run binary not found"),
            "error message should explain what is missing"
        );
    }
}
