use std::path::Path;

use super::types::LinkKind;

/// Determine which link type to use based on filesystem boundaries.
///
/// Uses the device ID (`st_dev`) to detect if source and destination are on
/// the same filesystem. Hardlinks are preferred when possible because they:
/// - Are faster to create
/// - Cannot dangle (inode-based)
/// - Survive renames of the store directory
///
/// Falls back to symlinks if filesystems differ or if device check fails.
#[cfg(unix)]
pub fn determine_link_type(src: &Path, dest_dir: &Path) -> LinkKind {
    use std::os::unix::fs::MetadataExt;

    let src_dev = std::fs::metadata(src)
        .map(|m| m.dev())
        .unwrap_or(0);
    let dest_dev = std::fs::metadata(dest_dir)
        .map(|m| m.dev())
        .unwrap_or(1); // Different default to force symlink on failure

    if src_dev != 0 && src_dev == dest_dev {
        LinkKind::Hardlink
    } else {
        LinkKind::Symlink
    }
}

#[cfg(not(unix))]
pub fn determine_link_type(_src: &Path, _dest_dir: &Path) -> LinkKind {
    LinkKind::Symlink
}
