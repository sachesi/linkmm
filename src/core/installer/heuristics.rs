use std::collections::HashSet;
use std::path::Path;

use super::archive::{list_archive_entries_with_7z, read_archive_files_bytes};
use super::paths::{
    has_zip_extension, installer_log_warning,
    normalize_path_lowercase,
};
use super::types::{
    DataArchivePlan, InstallStrategy,
    KNOWN_DATA_SUBDIRS, KNOWN_PLUGIN_EXTS, KNOWN_ARCHIVE_EXTS, JUNK_TOPLEVEL_ENTRIES,
};

/// Score a path prefix as a candidate game Data-root directory.
///
/// Higher scores indicate a better match.  Scoring weights:
/// * **+20** if the directory itself is named `data` (case-insensitive)
/// * **+10** for each direct-child directory that matches a known game
///   data subdirectory (meshes, textures, scripts, …)
/// * **+15** for each direct-child file with a plugin extension
///   (.esp / .esm / .esl)
pub(super) fn score_as_data_root(prefix: &str, paths: &[&str]) -> i32 {
    let mut score = 0i32;

    let prefix_norm = prefix.to_lowercase().replace('\\', "/");
    let dir_name = prefix_norm
        .trim_end_matches('/')
        .split('/')
        .next_back()
        .unwrap_or("");
    if dir_name == "data" {
        score += 20;
    }

    let search_prefix: String = if prefix_norm.trim_end_matches('/').is_empty() {
        String::new()
    } else {
        format!("{}/", prefix_norm.trim_end_matches('/'))
    };

    let mut seen: HashSet<String> = HashSet::new();

    for path in paths {
        let path_norm = path.to_lowercase().replace('\\', "/");
        let path_norm = path_norm.trim_start_matches('/');

        let rel = if !search_prefix.is_empty() {
            match path_norm.strip_prefix(&search_prefix) {
                Some(r) => r,
                None => continue,
            }
        } else {
            path_norm
        };

        let rel = rel.trim_start_matches('/');
        if rel.is_empty() {
            continue;
        }

        let first = rel.split('/').next().unwrap_or("").trim_end_matches('\\');
        if first.is_empty() || seen.contains(first) {
            continue;
        }
        seen.insert(first.to_string());

        if KNOWN_DATA_SUBDIRS.contains(&first) {
            score += 10;
        }

        if first.contains('.') {
            let ext = first.rsplit('.').next().unwrap_or("");
            if KNOWN_PLUGIN_EXTS.contains(&ext) {
                score += 15;
            }
        }
    }

    score
}

/// Score a path prefix using `Vec<String>` paths (from installer_new).
pub(super) fn score_as_data_root_owned(prefix: &str, paths: &[String]) -> i32 {
    let mut score = 0i32;

    let prefix_norm = normalize_path_lowercase(prefix);
    let dir_name = prefix_norm
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("");

    if dir_name == "data" {
        score += 20;
    }

    let search_prefix = if prefix_norm.is_empty() {
        String::new()
    } else {
        format!("{}/", prefix_norm.trim_end_matches('/'))
    };

    let mut seen = HashSet::new();

    for path in paths {
        let path_norm = normalize_path_lowercase(path);

        let rel = if !search_prefix.is_empty() {
            match path_norm.strip_prefix(&search_prefix) {
                Some(r) => r,
                None => continue,
            }
        } else {
            &path_norm
        };

        let rel = rel.trim_start_matches('/');
        if rel.is_empty() {
            continue;
        }

        let first_component = rel.split('/').next().unwrap_or("");
        if first_component.is_empty() {
            continue;
        }

        if !seen.insert(first_component.to_string()) {
            continue;
        }

        if KNOWN_DATA_SUBDIRS.contains(&first_component) {
            score += 10;
        }

        for ext in KNOWN_PLUGIN_EXTS {
            if first_component.ends_with(&format!(".{}", ext)) {
                score += 15;
                break;
            }
        }

        for ext in KNOWN_ARCHIVE_EXTS {
            if first_component.ends_with(&format!(".{}", ext)) {
                score += 5;
                break;
            }
        }
    }

    score
}

/// Find the directory that directly contains the `fomod/` subdirectory.
pub(super) fn find_fomod_parent_dir(paths: &[&str]) -> Option<String> {
    for path in paths {
        let norm = path.to_lowercase().replace('\\', "/");
        let norm = norm.trim_start_matches('/');
        if norm == "fomod/moduleconfig.xml" {
            log::debug!(
                "[FOMOD] ModuleConfig.xml discovered at archive root | virtual_path={}",
                path
            );
            return Some(String::new());
        }
        if norm.ends_with("/fomod/moduleconfig.xml") {
            let orig = path.replace('\\', "/");
            let orig = orig.trim_start_matches('/');
            let parent = orig.split('/').next().unwrap_or("").to_string();
            log::debug!(
                "[FOMOD] ModuleConfig.xml discovered under wrapper | virtual_path={}, parent_dir={}",
                path,
                parent
            );
            return Some(parent);
        }
    }
    None
}

/// Scan all paths in an archive and return the prefix to strip.
pub fn find_data_root_in_paths(paths: &[&str]) -> String {
    if let Some(fomod_parent) = find_fomod_parent_dir(paths) {
        let result = if fomod_parent.is_empty() {
            String::new()
        } else {
            format!("{fomod_parent}/")
        };
        log::debug!(
            "[DataRoot] FOMOD detected, using fomod parent as root | fomod_parent='{}', resolved_root='{}'",
            fomod_parent,
            result
        );
        return result;
    }

    let paths_owned: Vec<String> = paths.iter().map(|s| s.to_string()).collect();

    match detect_data_root(&paths_owned) {
        Some(prefix) => {
            let result = if prefix.is_empty() {
                String::new()
            } else {
                format!("{}/", prefix)
            };
            log::debug!(
                "[DataRoot] Resolved via scoring heuristic | detected_prefix='{}', strip_prefix='{}'",
                prefix,
                result
            );
            result
        }
        None => {
            let fallback = find_common_prefix_from_paths(paths);
            log::debug!(
                "[DataRoot] Scoring heuristic inconclusive, fallback to common prefix | strip_prefix='{}'",
                fallback
            );
            fallback
        }
    }
}

/// Compute the common single-level top-directory prefix.
///
/// Entries whose top-level component is in `JUNK_TOPLEVEL_DIRS` (e.g.
/// `__MACOSX/`) are ignored.  Comparison is case-insensitive to handle minor
/// casing differences in the wrapper-dir name across entries.
pub(super) fn find_common_prefix_from_paths(paths: &[&str]) -> String {
    if paths.is_empty() {
        return String::new();
    }

    let mut first_top: Option<String> = None;
    let mut all_same = true;

    for path in paths {
        let p = path.replace('\\', "/");
        let top = p.trim_start_matches('/').split('/').next().unwrap_or("");
        if top.is_empty() {
            continue;
        }
        if JUNK_TOPLEVEL_ENTRIES.contains(&top.to_lowercase().as_str()) {
            continue;
        }
        match &first_top {
            None => first_top = Some(top.to_string()),
            Some(ft) if ft.to_lowercase() != top.to_lowercase() => {
                all_same = false;
                break;
            }
            _ => {}
        }
    }

    if all_same
        && let Some(ft) = first_top
            && paths.len() > 1 {
                return format!("{ft}/");
            }
    String::new()
}

/// Return `true` if the paths look like game-root-level content.
fn looks_like_root_level_mod(paths: &[&str]) -> bool {
    const ROOT_DIRS: &[&str] = &["enbseries", "reshade-shaders", "reshade"];
    const ROOT_EXTS: &[&str] = &["dll", "asi"];

    for path in paths {
        let p = path.to_lowercase().replace('\\', "/");
        let first = p.trim_start_matches('/').split('/').next().unwrap_or("");
        if first.is_empty() {
            continue;
        }
        if ROOT_DIRS.contains(&first) {
            log::debug!(
                "[DataRoot] Root-level mod detected via directory marker | dir={}",
                first
            );
            return true;
        }
        if first.contains('.') {
            let ext = first.rsplit('.').next().unwrap_or("");
            if ROOT_EXTS.contains(&ext) {
                log::debug!(
                    "[DataRoot] Root-level mod detected via binary extension | file={}",
                    first
                );
                return true;
            }
        }
    }
    false
}

pub(super) fn build_data_archive_plan(path_refs: &[&str]) -> DataArchivePlan {
    if is_bain_archive(path_refs) {
        return DataArchivePlan::Bain {
            top_dirs: collect_bain_top_dirs(path_refs),
        };
    }

    let data_root = find_data_root_in_paths(path_refs);
    let root_trimmed = data_root.trim_end_matches('/');
    let best_score = score_as_data_root(root_trimmed, path_refs);
    if best_score > 0 {
        return DataArchivePlan::ExtractToData {
            strip_prefix: data_root,
        };
    }

    let simple_prefix = find_common_prefix_from_paths(path_refs);
    let stripped: Vec<String> = path_refs
        .iter()
        .filter_map(|p| {
            let p_lower = p.to_lowercase().replace('\\', "/");
            let p_lower = p_lower.trim_start_matches('/').to_string();
            if simple_prefix.is_empty() {
                Some(p_lower)
            } else {
                p_lower
                    .strip_prefix(&simple_prefix.to_lowercase())
                    .map(|s| s.to_string())
            }
        })
        .collect();
    let stripped_refs: Vec<&str> = stripped.iter().map(|s| s.as_str()).collect();
    if looks_like_root_level_mod(&stripped_refs) {
        DataArchivePlan::ExtractToModRoot {
            strip_prefix: simple_prefix,
        }
    } else {
        DataArchivePlan::ExtractToData {
            strip_prefix: simple_prefix,
        }
    }
}

/// Return `true` if the archive follows BAIN package conventions.
pub fn is_bain_archive(paths: &[&str]) -> bool {
    let mut top_dirs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for path in paths {
        let p = path.replace('\\', "/");
        let p = p.trim_start_matches('/').to_string();
        if let Some(first) = p.split('/').next()
            && !first.is_empty() {
                top_dirs.insert(first.to_lowercase());
            }
    }

    if top_dirs.len() < 2 {
        return false;
    }

    top_dirs.iter().all(|d| {
        d.len() >= 3
            && d.chars().take(2).all(|c| c.is_ascii_digit())
            && matches!(d.chars().nth(2), Some(' ') | Some('_'))
    })
}

/// Return the sorted list of BAIN package directory names.
pub(super) fn collect_bain_top_dirs(paths: &[&str]) -> Vec<String> {
    let mut dirs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for path in paths {
        let p = path.replace('\\', "/");
        let p_lower = p.trim_start_matches('/').to_lowercase();
        if let Some(first) = p_lower.split('/').next()
            && !first.is_empty()
                && first.len() >= 3
                && first.chars().take(2).all(|c| c.is_ascii_digit())
                && matches!(first.chars().nth(2), Some(' ') | Some('_'))
            {
                let orig_p = p.trim_start_matches('/');
                if let Some(orig_first) = orig_p.split('/').next() {
                    dirs.insert(orig_first.to_string());
                }
            }
    }
    dirs.into_iter().collect()
}

/// Determine install strategy for an archive.
pub fn detect_strategy(archive_path: &Path) -> Result<InstallStrategy, String> {
    if !archive_path.exists() {
        return Err(format!("Cannot open archive: {}", archive_path.display()));
    }

    let archive_name = archive_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    if is_fomod_archive(archive_path) {
        log::debug!(
            "[Strategy] FOMOD installer detected | archive={}",
            archive_name
        );
        Ok(InstallStrategy::Fomod(vec![]))
    } else {
        log::debug!(
            "[Strategy] Standard data mod detected | archive={}",
            archive_name
        );
        Ok(InstallStrategy::Data)
    }
}

fn is_fomod_archive(archive_path: &Path) -> bool {
    let entries = if has_zip_extension(archive_path) {
        if let Ok(file) = std::fs::File::open(archive_path) {
            if let Ok(zip) = zip::ZipArchive::new(file) {
                zip.file_names().map(|s| s.to_string()).collect::<Vec<_>>()
            } else {
                vec![]
            }
        } else {
            vec![]
        }
    } else {
        list_archive_entries_with_7z(archive_path).unwrap_or_else(|e| {
            installer_log_warning(format!(
                "Failed to list entries in {}: {e}; assuming not a FOMOD archive",
                archive_path.display()
            ));
            vec![]
        })
    };

    if entries.is_empty() {
        return false;
    }

    if entries.iter().any(|p| {
        let lower = p.to_lowercase().replace('\\', "/");
        lower == "fomod/moduleconfig.xml" || lower.ends_with("/fomod/moduleconfig.xml")
    }) {
        return true;
    }

    let xml_files: Vec<&str> = entries
        .iter()
        .filter(|p| p.to_lowercase().ends_with(".xml"))
        .map(|s| s.as_str())
        .collect();

    if xml_files.is_empty() {
        return false;
    }

    if let Ok(xml_contents) = read_archive_files_bytes(archive_path, &xml_files) {
        for bytes in xml_contents.values() {
            if let Ok(content) = std::str::from_utf8(bytes) {
                let lower_content = content.to_lowercase();
                if lower_content.contains("<modulename>")
                    && (lower_content.contains("<installsteps>")
                        || lower_content.contains("<requiredinstallfiles>"))
                {
                    return true;
                }
            }
        }
    }

    false
}

#[allow(dead_code)]
fn top_level_component(path: &str) -> &str {
    let stripped = path.strip_prefix('/').unwrap_or(path);
    stripped.split('/').next().unwrap_or("")
}

/// Return `true` when the archive contains a `Data/` folder after prefix stripping.
#[allow(dead_code)]
pub(super) fn archive_has_data_folder(archive_path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(archive_path) else {
        return false;
    };
    let Ok(mut zip) = zip::ZipArchive::new(file) else {
        return false;
    };
    let prefix = super::archive::find_common_prefix(&zip);

    for i in 0..zip.len() {
        let Ok(entry) = zip.by_index(i) else {
            continue;
        };
        let name = entry.name();
        let rel = if !prefix.is_empty() {
            name.strip_prefix(&prefix).unwrap_or(name)
        } else {
            name
        };
        let rel_lower = rel.to_lowercase();
        let top = top_level_component(&rel_lower);
        if top == "data" {
            return true;
        }
    }
    false
}

/// Detect the Data/ root directory within an archive (from installer_new).
pub(super) fn detect_data_root(paths: &[String]) -> Option<String> {
    if paths.is_empty() {
        return None;
    }

    let mut top_level_dirs = HashSet::new();
    let mut has_root_level_data_indicators = false;

    for path in paths {
        let normalized = normalize_path_lowercase(path);
        let first_component = normalized.split('/').next().unwrap_or("");

        if first_component.is_empty() {
            continue;
        }

        top_level_dirs.insert(first_component.to_string());

        if KNOWN_DATA_SUBDIRS.contains(&first_component) {
            has_root_level_data_indicators = true;
        }

        for ext in KNOWN_PLUGIN_EXTS {
            if first_component.ends_with(&format!(".{}", ext)) {
                has_root_level_data_indicators = true;
                break;
            }
        }
    }

    log::debug!(
        "[DataRoot] Scanning archive | top_level_dirs={}, total_paths={}",
        top_level_dirs.len(),
        paths.len()
    );

    if has_root_level_data_indicators {
        log::debug!("[DataRoot] Root level already has data indicators, no prefix to strip");
        return Some(String::new());
    }

    if top_level_dirs.len() == 1 {
        let candidate = top_level_dirs.iter().next().unwrap();
        let score = score_as_data_root_owned(candidate, paths);

        log::debug!(
            "[DataRoot] Single wrapper directory | candidate={}, score={}",
            candidate,
            score
        );

        if score >= 10 {
            return Some(candidate.clone());
        }
    }

    let mut best_score = 0i32;
    let mut best_prefix = String::new();

    let root_score = score_as_data_root_owned("", paths);
    if root_score > 0 {
        best_score = root_score;
        best_prefix = String::new();
    }

    for dir in &top_level_dirs {
        let score = score_as_data_root_owned(dir, paths);
        log::debug!(
            "[DataRoot] Candidate scoring | prefix='{}', score={}",
            dir,
            score
        );
        if score > best_score {
            best_score = score;
            best_prefix = dir.clone();
        }
    }

    if best_score >= 10 {
        log::debug!(
            "[DataRoot] Best candidate selected | prefix='{}', score={}",
            best_prefix,
            best_score
        );
        Some(best_prefix)
    } else {
        log::debug!(
            "[DataRoot] No candidate met threshold | best_score={}, threshold=10",
            best_score
        );
        None
    }
}
