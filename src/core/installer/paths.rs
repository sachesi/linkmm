use std::path::Path;

/// Normalize a path component for safe filesystem operations.
///
/// - Converts backslashes to forward slashes
/// - Strips leading slashes
/// - Trims trailing slashes
pub(super) fn normalize_path(p: &str) -> String {
    let s = p.replace('\\', "/");
    let s = s.strip_prefix('/').unwrap_or(&s);
    s.trim_end_matches('/').to_string()
}

/// Normalize a path to lowercase for case-insensitive comparison.
///
/// **Critical for Linux**: The game engine is case-insensitive but the
/// filesystem is not.  We must normalize all Data/-relative paths to lowercase
/// before creating links to ensure consistency.
pub(super) fn normalize_path_lowercase(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

/// Strip a leading `Data/` segment (case-insensitive) from a path.
///
/// FOMOD `destination` attributes are relative to the **game root**, so a value
/// of `"Data/textures"` means the game's `Data/textures` folder.  Because the
/// installer already extracts into `mod_dir/Data/`, including the `Data/`
/// segment verbatim would produce `mod_dir/Data/Data/textures/` — the classic
/// double-nesting bug.  Stripping the leading `Data/` avoids this.
pub(super) fn strip_data_prefix(s: &str) -> String {
    let lower = s.to_lowercase();
    if lower == "data" || lower == "data/" {
        String::new()
    } else if lower.starts_with("data/") {
        s["data/".len()..].to_string()
    } else {
        s.to_string()
    }
}

/// Check that a relative path is safe (no traversal above the root).
///
/// Rejects paths containing `..` components that would escape the destination.
pub(super) fn is_safe_relative_path(path: &str) -> bool {
    use std::path::Component;
    let normalized = path.replace('\\', "/");
    let p = Path::new(&normalized);
    let mut depth: i32 = 0;
    for component in p.components() {
        match component {
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            Component::Normal(_) => {
                depth += 1;
            }
            Component::RootDir | Component::Prefix(_) => {
                // Absolute paths are not safe relative paths
                return false;
            }
            Component::CurDir => {}
        }
    }
    true
}

pub(super) fn has_zip_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("zip"))
        .unwrap_or(false)
}

pub(super) fn has_rar_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("rar"))
        .unwrap_or(false)
}

pub(super) fn installer_log_activity(message: impl AsRef<str>) {
    log::info!("{}", message.as_ref());
}

pub(super) fn installer_log_warning(message: impl AsRef<str>) {
    log::warn!("{}", message.as_ref());
}
