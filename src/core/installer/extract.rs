use std::io::BufWriter;
use std::path::{Path, PathBuf};

use super::archive::open_7z_reader;
use super::paths::{
    has_rar_extension, installer_log_warning,
    is_safe_relative_path, normalize_path,
};
use super::types::{EXTRACT_BUFFER_SIZE, EXTRACTION_TICK_INTERVAL_MS, SINGLE_FILE_READ_CAP};

/// Extract all files from a zip archive into `dest_dir`, stripping
/// `strip_prefix` from every entry name.
pub(super) fn extract_zip_to(
    archive_path: &Path,
    dest_dir: &Path,
    strip_prefix: &str,
    tick: &dyn Fn(),
) -> Result<(), String> {
    let file =
        std::fs::File::open(archive_path).map_err(|e| format!("Cannot open archive: {e}"))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| format!("Cannot read zip archive: {e}"))?;

    let mut last_tick = std::time::Instant::now();
    for i in 0..zip.len() {
        let now = std::time::Instant::now();
        if now.duration_since(last_tick).as_millis() as u64 >= EXTRACTION_TICK_INTERVAL_MS {
            tick();
            last_tick = now;
        }

        let mut entry = zip
            .by_index(i)
            .map_err(|e| format!("Cannot read zip entry: {e}"))?;

        let raw_name = entry.name().to_string();
        let rel_name = if !strip_prefix.is_empty() {
            match raw_name.strip_prefix(strip_prefix) {
                Some(r) => r.to_string(),
                None => {
                    let raw_lower = raw_name.to_lowercase();
                    let prefix_lower = strip_prefix.to_lowercase();
                    if let Some(r) = raw_lower.strip_prefix(&prefix_lower) {
                        raw_name[raw_name.len() - r.len()..].to_string()
                    } else {
                        continue;
                    }
                }
            }
        } else {
            raw_name
        };

        if rel_name.is_empty() || rel_name == "/" {
            continue;
        }

        if !is_safe_relative_path(&rel_name) {
            installer_log_warning(format!("Skipping zip entry with unsafe path: {rel_name}"));
            continue;
        }

        let out_path = dest_dir.join(&rel_name);

        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)
                .map_err(|e| format!("Failed to create directory {}: {e}", out_path.display()))?;
        } else {
            log::trace!("[Extract] zip entry | path={}", rel_name);
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create parent dir: {e}"))?;
            }
            let out_file = std::fs::File::create(&out_path)
                .map_err(|e| format!("Failed to create file {}: {e}", out_path.display()))?;
            let mut buffered = BufWriter::with_capacity(EXTRACT_BUFFER_SIZE, out_file);
            std::io::copy(&mut entry, &mut buffered)
                .map_err(|e| format!("Failed to extract {}: {e}", rel_name))?;
        }
    }

    Ok(())
}

/// Full-archive extraction for non-zip archives.
pub(super) fn extract_archive_with_7z(archive_path: &Path, dest_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest_dir).map_err(|e| {
        format!(
            "Failed to create extraction directory {}: {e}",
            dest_dir.display()
        )
    })?;

    if has_rar_extension(archive_path) {
        extract_rar_archive(archive_path, dest_dir)
    } else {
        extract_7z_archive(archive_path, dest_dir)
    }
}

fn extract_7z_archive(archive_path: &Path, dest_dir: &Path) -> Result<(), String> {
    sevenz_rust2::decompress_file(archive_path, dest_dir).map_err(|e| {
        format!(
            "Failed to extract 7z archive {}: {e}",
            archive_path.display()
        )
    })
}

/// Stream-extract a `.7z` archive directly to `dest_dir`, stripping `strip_prefix`.
pub(super) fn extract_7z_archive_to(
    archive_path: &Path,
    dest_dir: &Path,
    strip_prefix: &str,
    tick: &dyn Fn(),
) -> Result<(), String> {
    std::fs::create_dir_all(dest_dir).map_err(|e| {
        format!(
            "Failed to create extraction directory {}: {e}",
            dest_dir.display()
        )
    })?;

    let prefix_lower = strip_prefix.to_lowercase().replace('\\', "/");
    let dest_dir_buf = dest_dir.to_path_buf();
    let mut last_tick = std::time::Instant::now();

    sevenz_rust2::decompress_file_with_extract_fn(
        archive_path,
        dest_dir,
        |entry, reader, _default_dest| {
            let now = std::time::Instant::now();
            if now.duration_since(last_tick).as_millis() as u64 >= EXTRACTION_TICK_INTERVAL_MS {
                tick();
                last_tick = now;
            }

            let raw_name = normalize_path(entry.name());

            let rel_name = if prefix_lower.is_empty() {
                raw_name.clone()
            } else {
                let raw_lower = raw_name.to_lowercase().replace('\\', "/");
                match raw_lower.strip_prefix(&prefix_lower) {
                    Some(r) => raw_name[raw_name.len() - r.len()..].to_string(),
                    None => return Ok(true),
                }
            };

            let rel_name = rel_name.trim_start_matches('/').to_string();
            if rel_name.is_empty() {
                return Ok(true);
            }

            if !is_safe_relative_path(&rel_name) {
                installer_log_warning(format!("Skipping 7z entry with unsafe path: {rel_name}"));
                return Ok(true);
            }

            let out_path = dest_dir_buf.join(&rel_name);

            if entry.is_directory() {
                std::fs::create_dir_all(&out_path)?;
            } else {
                log::trace!("[Extract] 7z entry | path={}", rel_name);
                if let Some(parent) = out_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let out_file = std::fs::File::create(&out_path)?;
                let mut buffered = BufWriter::with_capacity(EXTRACT_BUFFER_SIZE, out_file);
                std::io::copy(reader, &mut buffered)?;
            }

            Ok(true)
        },
    )
    .map_err(|e| {
        format!(
            "Failed to extract 7z archive {}: {e}",
            archive_path.display()
        )
    })
}

/// Extract a `.rar` archive to `dest_dir` with prefix stripping.
pub(super) fn extract_rar_archive_to(
    archive_path: &Path,
    dest_dir: &Path,
    strip_prefix: &str,
) -> Result<(), String> {
    std::fs::create_dir_all(dest_dir).map_err(|e| {
        format!(
            "Failed to create extraction directory {}: {e}",
            dest_dir.display()
        )
    })?;

    let tmp = create_temp_extract_dir_in(dest_dir)?;

    extract_rar_archive(archive_path, &tmp)?;

    let result = (|| {
        if strip_prefix.is_empty() {
            return move_dir_contents(&tmp, dest_dir);
        }
        let prefix_trimmed = strip_prefix.trim_end_matches('/');
        let mut src = tmp.clone();
        for component in prefix_trimmed.split('/') {
            if !component.is_empty() {
                src = src.join(component);
            }
        }
        if src.is_dir() {
            move_dir_contents(&src, dest_dir)
        } else {
            move_dir_contents(&tmp, dest_dir)
        }
    })();

    if let Err(e) = std::fs::remove_dir_all(&tmp) {
        installer_log_warning(format!(
            "Failed to remove temporary extraction directory {}: {e}",
            tmp.display()
        ));
    }

    result
}

/// Dispatch prefix-stripped extraction for a non-zip archive.
pub(super) fn extract_non_zip_to(
    archive_path: &Path,
    dest_dir: &Path,
    strip_prefix: &str,
    tick: &dyn Fn(),
) -> Result<(), String> {
    if has_rar_extension(archive_path) {
        extract_rar_archive_to(archive_path, dest_dir, strip_prefix)
    } else {
        extract_7z_archive_to(archive_path, dest_dir, strip_prefix, tick)
    }
}

/// Extract a `.rar` archive to `dest_dir` using `unrar`.
pub(super) fn extract_rar_archive(archive_path: &Path, dest_dir: &Path) -> Result<(), String> {
    let mut archive = unrar::Archive::new(archive_path)
        .open_for_processing()
        .map_err(|e| format!("Failed to open RAR archive {}: {e}", archive_path.display()))?;
    loop {
        match archive.read_header() {
            Err(e) => return Err(format!("Failed to read RAR header: {e}")),
            Ok(None) => break,
            Ok(Some(header)) => {
                archive = header
                    .extract_with_base(dest_dir)
                    .map_err(|e| format!("Failed to extract RAR entry: {e}"))?;
            }
        }
    }
    Ok(())
}

/// Extract a single file from a non-zip archive.
#[allow(dead_code)]
pub(super) fn extract_single_file_with_7z(
    archive_path: &Path,
    file_path_in_archive: &str,
    dest_dir: &Path,
) -> Result<(), String> {
    std::fs::create_dir_all(dest_dir).map_err(|e| {
        format!(
            "Failed to create extraction directory {}: {e}",
            dest_dir.display()
        )
    })?;

    if has_rar_extension(archive_path) {
        extract_single_rar_file(archive_path, file_path_in_archive, dest_dir)
    } else {
        extract_single_7z_file(archive_path, file_path_in_archive, dest_dir)
    }
}

/// Extract a single file from a `.7z` archive.
#[allow(dead_code)]
fn extract_single_7z_file(
    archive_path: &Path,
    file_path_in_archive: &str,
    dest_dir: &Path,
) -> Result<(), String> {
    let target_norm = normalize_path(file_path_in_archive);
    let target_lower = target_norm.to_lowercase();

    let mut reader = open_7z_reader(archive_path)?;

    let data = match reader.read_file(file_path_in_archive) {
        Ok(bytes) => {
            log::debug!(
                "[Extract7z] read_file exact match succeeded | path={}",
                file_path_in_archive
            );
            bytes
        }
        Err(_) => {
            match reader.read_file(&target_norm) {
                Ok(bytes) => {
                    log::debug!(
                        "[Extract7z] read_file normalised match succeeded | path={}",
                        target_norm
                    );
                    bytes
                }
                Err(_) => {
                    log::debug!(
                        "[Extract7z] read_file failed, falling back to sequential scan | target={}",
                        target_lower
                    );
                    let mut found = None;
                    reader
                        .for_each_entries(|entry, entry_reader| {
                            let entry_norm = normalize_path(entry.name());
                            let entry_lower = entry_norm.to_lowercase();
                            if entry_lower == target_lower && !entry.is_directory() {
                                let cap = std::cmp::min(entry.size() as usize, SINGLE_FILE_READ_CAP);
                                let mut buf = Vec::with_capacity(cap);
                                if entry_reader.read_to_end(&mut buf).is_ok() {
                                    found = Some(buf);
                                    return Ok(false);
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
                    found.ok_or_else(|| {
                        format!(
                            "Failed to read '{file_path_in_archive}' from {}",
                            archive_path.display()
                        )
                    })?
                }
            }
        }
    };

    let out_path = dest_dir.join(Path::new(&target_norm));
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create parent directory: {e}"))?;
    }
    std::fs::write(&out_path, &data)
        .map_err(|e| format!("Failed to write extracted file {}: {e}", out_path.display()))
}

/// Extract a single file from a `.rar` archive.
pub(super) fn extract_single_rar_file(
    archive_path: &Path,
    file_path_in_archive: &str,
    dest_dir: &Path,
) -> Result<(), String> {
    let target_lower = file_path_in_archive.to_lowercase().replace('\\', "/");
    let mut archive = unrar::Archive::new(archive_path)
        .open_for_processing()
        .map_err(|e| format!("Failed to open RAR archive {}: {e}", archive_path.display()))?;
    loop {
        match archive.read_header() {
            Err(e) => return Err(format!("Failed to read RAR header: {e}")),
            Ok(None) => break,
            Ok(Some(header)) => {
                let entry_name = header.entry().filename.to_string_lossy().to_string();
                let entry_lower = entry_name.to_lowercase().replace('\\', "/");
                if entry_lower == target_lower {
                    header
                        .extract_with_base(dest_dir)
                        .map_err(|e| format!("Failed to extract RAR entry '{entry_name}': {e}"))?;
                    return Ok(());
                }
                archive = header
                    .skip()
                    .map_err(|e| format!("Failed to skip RAR entry '{entry_name}': {e}"))?;
            }
        }
    }
    Err(format!(
        "File '{file_path_in_archive}' not found in RAR archive {}",
        archive_path.display()
    ))
}

pub(super) fn create_temp_extract_dir() -> Result<PathBuf, String> {
    let base = std::env::temp_dir();
    for attempt in 0..100u32 {
        let temp_extract_path = base.join(format!(
            "linkmm_extract_{}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            attempt
        ));
        match std::fs::create_dir(&temp_extract_path) {
            Ok(()) => return Ok(temp_extract_path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(format!(
                    "Failed to create temporary extraction directory {}: {e}",
                    temp_extract_path.display()
                ));
            }
        }
    }
    Err("Failed to allocate a temporary extraction directory".to_string())
}

pub(super) fn create_temp_extract_dir_in(parent: &Path) -> Result<PathBuf, String> {
    for attempt in 0..100u32 {
        let temp_extract_path = parent.join(format!(
            ".linkmm_tmp_{}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            attempt
        ));
        match std::fs::create_dir(&temp_extract_path) {
            Ok(()) => return Ok(temp_extract_path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(format!(
                    "Failed to create temporary extraction directory {}: {e}",
                    temp_extract_path.display()
                ));
            }
        }
    }
    Err("Failed to allocate a temporary extraction directory".to_string())
}

pub(super) fn move_dir_contents(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst)
        .map_err(|e| format!("Failed to create destination {}: {e}", dst.display()))?;
    for entry in
        std::fs::read_dir(src).map_err(|e| format!("Failed to read {}: {e}", src.display()))?
    {
        let entry = entry.map_err(|e| format!("Failed to read directory entry: {e}"))?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if std::fs::rename(&from, &to).is_err() {
            if from.is_dir() {
                copy_dir_contents(&from, &to)?;
                let _ = std::fs::remove_dir_all(&from);
            } else if from.is_file() {
                if let Some(parent) = to.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("Failed to create parent dir: {e}"))?;
                }
                std::fs::copy(&from, &to).map_err(|e| {
                    format!("Failed to move {} → {}: {e}", from.display(), to.display())
                })?;
                let _ = std::fs::remove_file(&from);
            }
        }
    }
    Ok(())
}

fn copy_dir_contents(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst)
        .map_err(|e| format!("Failed to create destination {}: {e}", dst.display()))?;
    for entry in
        std::fs::read_dir(src).map_err(|e| format!("Failed to read {}: {e}", src.display()))?
    {
        let entry = entry.map_err(|e| format!("Failed to read directory entry: {e}"))?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_contents(&from, &to)?;
        } else if from.is_file() {
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    format!("Failed to create parent dir {}: {e}", parent.display())
                })?;
            }
            std::fs::copy(&from, &to).map_err(|e| {
                format!(
                    "Failed to copy extracted file {} -> {}: {e}",
                    from.display(),
                    to.display()
                )
            })?;
        }
    }
    Ok(())
}

/// Recursively rename every file and directory inside `dir` to lowercase.
pub(super) fn normalize_paths_to_lowercase(dir: &Path) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) => {
            installer_log_warning(format!(
                "normalize_paths_to_lowercase: cannot read {}: {err}",
                dir.display()
            ));
            return;
        }
    };

    let items: Vec<_> = entries.flatten().collect();

    for entry in items {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let lower = name_str.to_lowercase();

        if name_str.as_ref() == lower.as_str() {
            if path.is_dir() {
                normalize_paths_to_lowercase(&path);
            }
            continue;
        }

        let new_path = dir.join(&lower);

        if new_path.exists() {
            if path.is_dir() && new_path.is_dir() {
                if let Ok(children) = std::fs::read_dir(&path) {
                    for child in children.flatten() {
                        let child_dst = new_path.join(child.file_name());
                        if let Err(e) = std::fs::rename(child.path(), &child_dst) {
                            installer_log_warning(format!(
                                "normalize_paths_to_lowercase: failed to merge {} -> {}: {e}",
                                child.path().display(),
                                child_dst.display()
                            ));
                        }
                    }
                }
                let _ = std::fs::remove_dir(&path);
                normalize_paths_to_lowercase(&new_path);
            }
        } else {
            if let Err(e) = std::fs::rename(&path, &new_path) {
                installer_log_warning(format!(
                    "normalize_paths_to_lowercase: failed to rename {} -> {}: {e}",
                    path.display(),
                    new_path.display()
                ));
                if path.is_dir() {
                    normalize_paths_to_lowercase(&path);
                }
                continue;
            }
            if new_path.is_dir() {
                normalize_paths_to_lowercase(&new_path);
            }
        }
    }
}
