// Retained only for the cross-filesystem move helper used in tests.
// Symlink-based deployment has been replaced by FUSE VFS (src/core/vfs.rs).

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
    fn move_file_creates_parent_dirs_and_moves() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("source.txt");
        let dest = temp.path().join("nested").join("dest.txt");
        fs::write(&src, b"payload").unwrap();
        move_file_with_cross_fs_fallback(&src, &dest, "move test file").unwrap();
        assert!(!src.exists());
        assert_eq!(fs::read(&dest).unwrap(), b"payload");
    }

    #[test]
    fn move_file_preserves_permissions() {
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
