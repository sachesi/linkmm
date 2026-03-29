use std::io::Read;
use std::path::Path;

use super::paths::{has_rar_extension, has_zip_extension, normalize_path};
use super::types::{FOMOD_DIR_PREFIX, JUNK_TOPLEVEL_ENTRIES, SINGLE_FILE_READ_CAP};

/// Open a 7z archive and return an `ArchiveReader` ready for reading.
pub(super) fn open_7z_reader(
    archive_path: &Path,
) -> Result<sevenz_rust2::ArchiveReader<std::fs::File>, String> {
    let file = std::fs::File::open(archive_path)
        .map_err(|e| format!("Cannot open archive {}: {e}", archive_path.display()))?;
    sevenz_rust2::ArchiveReader::new(file, sevenz_rust2::Password::empty())
        .map_err(|e| format!("Failed to read 7z archive {}: {e}", archive_path.display()))
}

/// List all file/directory paths stored inside a non-zip archive using native
/// Rust crates (no subprocess).
pub fn list_archive_entries_with_7z(archive_path: &Path) -> Result<Vec<String>, String> {
    if has_rar_extension(archive_path) {
        list_rar_entries(archive_path)
    } else {
        list_7z_entries(archive_path)
    }
}

/// List entries in a `.7z` archive.
fn list_7z_entries(archive_path: &Path) -> Result<Vec<String>, String> {
    let reader = open_7z_reader(archive_path)?;
    let paths = reader
        .archive()
        .files
        .iter()
        .map(|f| f.name().to_string())
        .collect();
    Ok(paths)
}

/// List entries in a `.rar` archive.
pub(super) fn list_rar_entries(archive_path: &Path) -> Result<Vec<String>, String> {
    let archive = unrar::Archive::new(archive_path)
        .open_for_listing()
        .map_err(|e| format!("Failed to open RAR archive {}: {e}", archive_path.display()))?;
    let mut paths = Vec::new();
    for entry in archive.flatten() {
        paths.push(entry.filename.to_string_lossy().to_string());
    }
    Ok(paths)
}

/// Load multiple files from an archive by path in a single pass.
/// Optimized for solid 7z archives to prevent multiple full sequential scans.
pub fn read_archive_files_bytes(
    archive_path: &Path,
    relative_paths: &[&str],
) -> Result<std::collections::HashMap<String, Vec<u8>>, String> {
    let mut results = std::collections::HashMap::new();

    if relative_paths.is_empty() {
        return Ok(results);
    }

    if !has_zip_extension(archive_path) {
        return read_archive_files_bytes_non_zip(archive_path, relative_paths);
    }

    let file =
        std::fs::File::open(archive_path).map_err(|e| format!("Cannot open archive: {e}"))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| format!("Cannot read zip archive: {e}"))?;

    for rel_path in relative_paths {
        let target = normalize_path(rel_path);
        let target_lower = target.to_lowercase();
        if target_lower.is_empty() {
            continue;
        }
        let fomod_target = format!("{FOMOD_DIR_PREFIX}{target_lower}");

        for i in 0..zip.len() {
            let mut entry = match zip.by_index(i) {
                Ok(e) => e,
                Err(_) => continue,
            };
            if entry.is_dir() {
                continue;
            }
            let name_norm = normalize_path(entry.name());
            let name_lower = name_norm.to_lowercase();
            let matches = name_lower == target_lower
                || name_lower.ends_with(&format!("/{target_lower}"))
                || name_lower == fomod_target
                || name_lower.ends_with(&format!("/{fomod_target}"));
            if matches {
                let mut bytes = Vec::new();
                if entry.read_to_end(&mut bytes).is_ok() {
                    results.insert(rel_path.to_string(), bytes);
                }
                break;
            }
        }
    }

    Ok(results)
}

/// Load a file from an archive by path, using case-insensitive matching and
/// common FOMOD-relative fallbacks.
#[allow(dead_code)]
pub fn read_archive_file_bytes(
    archive_path: &Path,
    relative_path: &str,
) -> Result<Vec<u8>, String> {
    if !has_zip_extension(archive_path) {
        return read_archive_file_bytes_non_zip(archive_path, relative_path);
    }

    let file =
        std::fs::File::open(archive_path).map_err(|e| format!("Cannot open archive: {e}"))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| format!("Cannot read zip archive: {e}"))?;

    let target = normalize_path(relative_path);
    let target_lower = target.to_lowercase();
    if target_lower.is_empty() {
        return Err("Empty archive path".to_string());
    }
    let fomod_target = format!("{FOMOD_DIR_PREFIX}{target_lower}");

    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| format!("Cannot read zip entry: {e}"))?;
        if entry.is_dir() {
            continue;
        }
        let name_norm = normalize_path(entry.name());
        let name_lower = name_norm.to_lowercase();
        let matches = name_lower == target_lower
            || name_lower.ends_with(&format!("/{target_lower}"))
            || name_lower == fomod_target
            || name_lower.ends_with(&format!("/{fomod_target}"));
        if matches {
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .map_err(|e| format!("Failed reading archive file {name_norm}: {e}"))?;
            return Ok(bytes);
        }
    }

    Err(format!("Archive file not found: {relative_path}"))
}

/// Read multiple files from a non-zip archive in a single pass.
fn read_archive_files_bytes_non_zip(
    archive_path: &Path,
    relative_paths: &[&str],
) -> Result<std::collections::HashMap<String, Vec<u8>>, String> {
    let entries = list_archive_entries_with_7z(archive_path)?;
    let mut target_to_req = std::collections::HashMap::new();

    for rel_path in relative_paths {
        let target = normalize_path(rel_path);
        let target_lower = target.to_lowercase();
        if target_lower.is_empty() {
            continue;
        }
        let fomod_target = format!("{FOMOD_DIR_PREFIX}{target_lower}");

        let matching_entry = entries.iter().find(|p| {
            let norm = normalize_path(p);
            let lower = norm.to_lowercase();
            lower == target_lower
                || lower.ends_with(&format!("/{target_lower}"))
                || lower == fomod_target
                || lower.ends_with(&format!("/{fomod_target}"))
        });

        if let Some(entry_path) = matching_entry {
            target_to_req.insert(entry_path.clone(), rel_path.to_string());
        }
    }

    if target_to_req.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    let mut results = std::collections::HashMap::new();

    if has_rar_extension(archive_path) {
        let tmp = super::extract::create_temp_extract_dir()?;
        for (entry_path, rel_path) in target_to_req {
            if super::extract::extract_single_rar_file(archive_path, &entry_path, &tmp).is_ok() {
                let extracted_path = tmp.join(Path::new(&normalize_path(&entry_path)));
                if let Ok(bytes) = std::fs::read(&extracted_path) {
                    results.insert(rel_path, bytes);
                }
            }
        }
        let _ = std::fs::remove_dir_all(&tmp);
    } else {
        let target_set: std::collections::HashMap<String, String> = target_to_req
            .iter()
            .map(|(entry, rel)| (normalize_path(entry).to_lowercase(), rel.clone()))
            .collect();
        let mut reader = open_7z_reader(archive_path)?;
        reader
            .for_each_entries(|entry, entry_reader| {
                let entry_lower = normalize_path(entry.name()).to_lowercase();
                if let Some(rel_path) = target_set.get(&entry_lower)
                    && !entry.is_directory() {
                        let cap = std::cmp::min(entry.size() as usize, SINGLE_FILE_READ_CAP);
                        let mut buf = Vec::with_capacity(cap);
                        if entry_reader.read_to_end(&mut buf).is_ok() {
                            results.insert(rel_path.clone(), buf);
                        }
                    }
                Ok(true)
            })
            .map_err(|e| {
                format!(
                    "Failed to read 7z archive {}: {e}",
                    archive_path.display()
                )
            })?;
    }

    Ok(results)
}

/// Read a file from a non-zip archive.
#[allow(dead_code)]
fn read_archive_file_bytes_non_zip(
    archive_path: &Path,
    relative_path: &str,
) -> Result<Vec<u8>, String> {
    let entries = list_archive_entries_with_7z(archive_path)?;

    let target = normalize_path(relative_path);
    let target_lower = target.to_lowercase();
    if target_lower.is_empty() {
        return Err("Empty archive path".to_string());
    }
    let fomod_target = format!("{FOMOD_DIR_PREFIX}{target_lower}");

    let matching_entry = entries.iter().find(|p| {
        let norm = normalize_path(p);
        let lower = norm.to_lowercase();
        lower == target_lower
            || lower.ends_with(&format!("/{target_lower}"))
            || lower == fomod_target
            || lower.ends_with(&format!("/{fomod_target}"))
    });

    let Some(entry_path) = matching_entry else {
        return Err(format!("Archive file not found: {relative_path}"));
    };

    if has_rar_extension(archive_path) {
        let tmp = super::extract::create_temp_extract_dir()?;
        let result = (|| {
            super::extract::extract_single_rar_file(archive_path, entry_path, &tmp)?;
            let normalised = normalize_path(entry_path);
            let extracted_path = tmp.join(Path::new(&normalised));
            std::fs::read(&extracted_path).map_err(|e| {
                format!(
                    "Failed to read extracted file {}: {e}",
                    extracted_path.display()
                )
            })
        })();
        let _ = std::fs::remove_dir_all(&tmp);
        result
    } else {
        let mut reader = open_7z_reader(archive_path)?;
        reader.read_file(entry_path).map_err(|e| {
            format!(
                "Failed to read '{}' from {}: {e}",
                entry_path,
                archive_path.display()
            )
        })
    }
}

/// Detect common top-level prefix shared by all zip entries.
///
/// Entries whose top-level component is in `JUNK_TOPLEVEL_DIRS` (e.g.
/// `__MACOSX/`) are ignored so that macOS resource-fork entries don't prevent
/// wrapper-directory detection.  Comparison is case-insensitive so minor
/// casing differences in repeated wrapper-dir names don't break detection.
pub(super) fn find_common_prefix(zip: &zip::ZipArchive<std::fs::File>) -> String {
    if zip.is_empty() {
        return String::new();
    }

    let mut first_top: Option<String> = None;
    let mut all_same = true;

    for name in zip.file_names() {
        let top = name.split('/').next().unwrap_or("");
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
        && let Some(ft) = first_top {
            if zip.len() > 1 {
                return format!("{ft}/");
            }
        }
    String::new()
}
