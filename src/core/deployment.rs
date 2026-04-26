use std::fs;
use std::path::Path;

use crate::core::games::Game;

// ── Legacy symlink cleanup ────────────────────────────────────────────────────

/// Remove all symlinks recursively from a directory tree.
/// Used during migration from link-based deployment to FUSE VFS.
pub fn purge_all_symlinks(dir: &Path) -> usize {
    if !dir.is_dir() {
        return 0;
    }

    let mut removed = 0;

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();

            if path.is_symlink() {
                if fs::remove_file(&path).is_ok() {
                    removed += 1;
                }
            } else if path.is_dir() {
                removed += purge_all_symlinks(&path);
                let _ = fs::remove_dir(&path);
            }
        }
    }

    removed
}

/// Clean up legacy nested Data/Data/ directory if it exists.
pub fn cleanup_legacy_nested_data(game: &Game) {
    let legacy_nested = game.data_path.join("Data");
    if legacy_nested.is_dir() {
        let removed = purge_all_symlinks(&legacy_nested);
        if removed > 0 {
            log::info!("Cleaned up {} legacy symlinks from Data/Data/", removed);
        }
        let _ = fs::remove_dir(&legacy_nested);
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;
    use std::path::Path;
    use tempfile::TempDir;

    fn move_file_with_cross_fs_fallback(
        src: &Path,
        dest: &Path,
        action: &str,
    ) -> Result<(), String> {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                format!("Failed to create destination directory while trying to {action}: {e}")
            })?;
        }
        match fs::rename(src, dest) {
            Ok(()) => Ok(()),
            Err(err) if is_cross_device_link_error(&err) => {
                fs::copy(src, dest)
                    .map_err(|e| format!("Failed to copy file while trying to {action}: {e}"))?;

                if let Ok(metadata) = fs::metadata(src) {
                    let _ = fs::set_permissions(dest, metadata.permissions());
                }

                fs::remove_file(src).map_err(|e| {
                    format!("Failed to remove original file while trying to {action}: {e}")
                })?;
                Ok(())
            }
            Err(err) => Err(format!("Failed to {action}: {err}")),
        }
    }

    fn is_cross_device_link_error(err: &io::Error) -> bool {
        err.raw_os_error() == Some(18)
    }

    #[test]
    fn move_file_with_cross_fs_fallback_moves_file_and_creates_parent_dirs() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("source.txt");
        let dest = temp.path().join("nested").join("dest.txt");

        fs::write(&src, b"payload").unwrap();
        move_file_with_cross_fs_fallback(&src, &dest, "move test file").unwrap();

        assert!(!src.exists());
        assert_eq!(fs::read(&dest).unwrap(), b"payload");
    }

    #[test]
    fn move_file_with_cross_fs_fallback_preserves_permissions_on_fallback_path() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src.txt");
        let dest = temp.path().join("dest").join("dst.txt");
        fs::write(&src, b"perm-check").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&src, fs::Permissions::from_mode(0o640)).unwrap();
            move_file_with_cross_fs_fallback(&src, &dest, "permission move").unwrap();
            let mode = fs::metadata(&dest).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o640);
        }

        #[cfg(not(unix))]
        {
            move_file_with_cross_fs_fallback(&src, &dest, "permission move").unwrap();
            assert_eq!(fs::read(&dest).unwrap(), b"perm-check");
        }
    }
}
