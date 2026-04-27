use std::path::Path;

/// Robustly remove a directory and all its contents.
/// Logs a warning if removal fails.
pub fn rm_rf(path: impl AsRef<Path>) {
    let path = path.as_ref();
    if !path.exists() {
        return;
    }
    if let Err(e) = std::fs::remove_dir_all(path) {
        log::warn!("Failed to remove directory {}: {e}", path.display());
    }
}

/// Robustly remove a file.
/// Logs a warning if removal fails.
pub fn rm_file(path: impl AsRef<Path>) {
    let path = path.as_ref();
    if !path.exists() {
        return;
    }
    if let Err(e) = std::fs::remove_file(path) {
        log::warn!("Failed to remove file {}: {e}", path.display());
    }
}
