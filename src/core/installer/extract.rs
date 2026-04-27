use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::archive::open_7z_reader;
use super::paths::{
    has_rar_extension, has_zip_extension, installer_log_warning, is_safe_relative_path,
    normalize_path,
};
use super::types::{
    EXTRACT_BUFFER_SIZE, EXTRACTION_TICK_INTERVAL_MS, JUNK_TOPLEVEL_ENTRIES, SINGLE_FILE_READ_CAP,
};

/// Extract all files from a zip archive into `dest_dir`, stripping
/// `strip_prefix` from every entry name.
///
/// `progress` is called periodically with `(bytes_written, total_bytes)`.
/// When `total_bytes` is zero the caller should treat progress as
/// indeterminate (pulse).
pub(super) fn extract_zip_to(
    archive_path: &Path,
    dest_dir: &Path,
    strip_prefix: &str,
    progress: &dyn Fn(u64, u64) -> bool,
) -> Result<(), String> {
    let file =
        std::fs::File::open(archive_path).map_err(|e| format!("Cannot open archive: {e}"))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| format!("Cannot read zip archive: {e}"))?;

    // Pre-scan the central directory to get the total uncompressed byte count
    // so we can report a real fraction instead of an indeterminate pulse.
    let mut total_bytes: u64 = 0;
    for i in 0..zip.len() {
        if let Ok(entry) = zip.by_index_raw(i) {
            total_bytes += entry.size();
        }
    }

    let mut bytes_done: u64 = 0;
    let mut last_tick = std::time::Instant::now();

    for i in 0..zip.len() {
        let now = std::time::Instant::now();
        if now.duration_since(last_tick).as_millis() as u64 >= EXTRACTION_TICK_INTERVAL_MS {
            if !progress(bytes_done, total_bytes) {
                return Err("Cancelled by user".to_string());
            }
            last_tick = now;
        }

        let mut entry = zip
            .by_index(i)
            .map_err(|e| format!("Cannot read zip entry: {e}"))?;

        let entry_size = entry.size();
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
                        bytes_done += entry_size;
                        continue;
                    }
                }
            }
        } else {
            raw_name
        };

        if rel_name.is_empty() || rel_name == "/" {
            bytes_done += entry_size;
            continue;
        }

        if !is_safe_relative_path(&rel_name) {
            installer_log_warning(format!("Skipping zip entry with unsafe path: {rel_name}"));
            bytes_done += entry_size;
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

        bytes_done += entry_size;
    }

    // Emit a final progress event at 100 % so the UI can settle.
    // Return value is intentionally ignored — extraction is already complete.
    let _ = progress(bytes_done, total_bytes);

    Ok(())
}

/// Full-archive extraction for non-zip archives (no prefix stripping).
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

/// Attempt extraction using the system `7z` binary for better performance.
///
/// Returns `Some(Ok(()))` on success, `Some(Err(…))` on failure or cancel,
/// or `None` when `7z` is not available and the caller should fall back to
/// the pure-Rust decompressor.
fn try_extract_with_system_7z(
    archive_path: &Path,
    dest_dir: &Path,
    progress: &dyn Fn(u64, u64) -> bool,
) -> Option<Result<(), String>> {
    use std::process::{Command, Stdio};

    let mut child = match Command::new("7z")
        .arg("x")
        .arg("-y")
        .arg(format!("-o{}", dest_dir.display()))
        .arg(archive_path.as_os_str())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => {
            log::debug!("[Extract] System 7z not available, using Rust library");
            return None;
        }
    };

    log::info!("[Extract] Using system 7z for faster extraction");

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    // Final 100 % progress tick.
                    let _ = progress(1, 1);
                    return Some(Ok(()));
                }
                let stderr = child
                    .stderr
                    .take()
                    .and_then(|mut s| {
                        let mut buf = String::new();
                        std::io::Read::read_to_string(&mut s, &mut buf).ok()?;
                        Some(buf)
                    })
                    .unwrap_or_default();
                return Some(Err(format!(
                    "System 7z extraction failed (exit {status}): {stderr}"
                )));
            }
            Ok(None) => {
                // Still running — report indeterminate progress and check
                // for cancellation.
                if !progress(0, 0) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Some(Err("Cancelled by user".to_string()));
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(e) => {
                return Some(Err(format!("Failed to wait for system 7z process: {e}")));
            }
        }
    }
}

/// Stream-extract a `.7z` archive directly to `dest_dir`, stripping
/// `strip_prefix` from every entry name.
///
/// Before starting decompression the archive header is read (no
/// decompression) to obtain the total uncompressed byte count.
/// `progress` is then called periodically with `(bytes_written,
/// total_bytes)` so callers can display a real percentage bar.
pub(super) fn extract_7z_archive_to(
    archive_path: &Path,
    dest_dir: &Path,
    strip_prefix: &str,
    progress: &dyn Fn(u64, u64) -> bool,
) -> Result<(), String> {
    std::fs::create_dir_all(dest_dir).map_err(|e| {
        format!(
            "Failed to create extraction directory {}: {e}",
            dest_dir.display()
        )
    })?;

    // When no prefix stripping is needed, try the system `7z` binary first.
    // It is typically 3–10× faster than the pure-Rust decompressor for large
    // LZMA2 solid archives.
    if strip_prefix.is_empty()
        && let Some(result) = try_extract_with_system_7z(archive_path, dest_dir, progress)
    {
        return result;
    }

    // Reading the archive header is cheap (no decompression) and gives us
    // the total uncompressed byte count for real progress reporting.
    let total_bytes: u64 = match sevenz_rust2::Archive::open(archive_path) {
        Ok(arch) => arch.files.iter().map(|e| e.size).sum(),
        Err(_) => 0,
    };

    let prefix_lower = strip_prefix.to_lowercase().replace('\\', "/");
    let dest_dir_buf = dest_dir.to_path_buf();
    let mut bytes_done: u64 = 0;
    let mut last_tick = std::time::Instant::now();

    sevenz_rust2::decompress_file_with_extract_fn(
        archive_path,
        dest_dir,
        |entry, reader, _default_dest| {
            let now = std::time::Instant::now();
            if now.duration_since(last_tick).as_millis() as u64 >= EXTRACTION_TICK_INTERVAL_MS {
                if !progress(bytes_done, total_bytes) {
                    return Err(sevenz_rust2::Error::Other(std::borrow::Cow::Borrowed(
                        "Cancelled by user",
                    )));
                }
                last_tick = now;
            }

            let raw_name = normalize_path(entry.name());

            let rel_name = if prefix_lower.is_empty() {
                raw_name.clone()
            } else {
                let raw_lower = raw_name.to_lowercase().replace('\\', "/");
                match raw_lower.strip_prefix(&prefix_lower) {
                    Some(r) => raw_name[raw_name.len() - r.len()..].to_string(),
                    None => {
                        bytes_done += entry.size();
                        return Ok(true);
                    }
                }
            };

            let rel_name = rel_name.trim_start_matches('/').to_string();
            if rel_name.is_empty() {
                bytes_done += entry.size();
                return Ok(true);
            }

            if !is_safe_relative_path(&rel_name) {
                installer_log_warning(format!("Skipping 7z entry with unsafe path: {rel_name}"));
                bytes_done += entry.size();
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

            bytes_done += entry.size();
            Ok(true)
        },
    )
    .map_err(|e| {
        let s = e.to_string();
        if s.contains("Cancelled by user") {
            "Cancelled by user".to_string()
        } else {
            format!(
                "Failed to extract 7z archive {}: {e}",
                archive_path.display()
            )
        }
    })?;

    // Emit a final progress event at 100 % so the UI can settle.
    // Return value is intentionally ignored — extraction is already complete.
    let _ = progress(bytes_done, total_bytes);

    Ok(())
}

/// Extract a `.rar` archive to `dest_dir` with prefix stripping.
///
/// Because the RAR library does not expose per-entry callbacks this function
/// extracts everything to a temporary subdirectory first and then moves the
/// relevant subtree into `dest_dir`.
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
            "Failed to remove temporary RAR extraction directory {}: {e}",
            tmp.display()
        ));
    }

    result
}

/// Dispatch prefix-stripped extraction for a non-zip archive.
///
/// `progress` is forwarded to the 7z extractor for real byte-based progress.
/// RAR extraction does not support per-entry callbacks and always reports
/// indeterminate progress.
pub(super) fn extract_non_zip_to(
    archive_path: &Path,
    dest_dir: &Path,
    strip_prefix: &str,
    progress: &dyn Fn(u64, u64) -> bool,
) -> Result<(), String> {
    if has_rar_extension(archive_path) {
        extract_rar_archive_to(archive_path, dest_dir, strip_prefix)
    } else {
        extract_7z_archive_to(archive_path, dest_dir, strip_prefix, progress)
    }
}

/// Extract a full `.rar` archive to `dest_dir`.
pub(super) fn extract_rar_archive(archive_path: &Path, dest_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest_dir).map_err(|e| {
        format!(
            "Failed to create extraction directory {}: {e}",
            dest_dir.display()
        )
    })?;

    let mut archive = unrar::Archive::new(archive_path)
        .open_for_processing()
        .map_err(|e| format!("Failed to open RAR archive {}: {e}", archive_path.display()))?;

    loop {
        match archive.read_header() {
            Err(e) => {
                return Err(format!(
                    "Failed to read RAR header in {}: {e}",
                    archive_path.display()
                ));
            }
            Ok(None) => break,
            Ok(Some(header)) => {
                archive = header.extract_with_base(dest_dir).map_err(|e| {
                    format!(
                        "Failed to extract entry from RAR archive {}: {e}",
                        archive_path.display()
                    )
                })?;
            }
        }
    }

    Ok(())
}

/// Extract a single file from either a `.7z` or `.rar` archive to `dest_dir`.
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
        Err(_) => match reader.read_file(&target_norm) {
            Ok(bytes) => {
                log::debug!(
                    "[Extract7z] read_file normalized match succeeded | path={}",
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
                        format!("Failed to read 7z archive {}: {e}", archive_path.display())
                    })?;
                found.ok_or_else(|| {
                    format!(
                        "Failed to read '{file_path_in_archive}' from {}",
                        archive_path.display()
                    )
                })?
            }
        },
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

/// Create a hidden temporary extraction directory inside `parent`.
///
/// The directory name starts with a `.` so it is hidden on Linux and clearly
/// identifiable as a transient artefact if cleanup ever fails.
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

/// Move all immediate children of `src` into `dst`, using `rename` when
/// possible and falling back to copy + delete for cross-device moves.
pub fn move_dir_contents(src: &Path, dst: &Path) -> Result<(), String> {
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

/// Recursively rename every file and directory under `dir` to lowercase.
///
/// This is required on Linux (case-sensitive filesystem) because the game
/// engine is case-insensitive: the deployment symlinks must match exactly the
/// lowercase paths that Bethesda titles use to look up assets.
///
/// **Rename order:** each entry is renamed before the function recurses into
/// it, so the recursion path is always valid.
pub(super) fn normalize_paths_to_lowercase(dir: &Path) {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };

    // Collect all entries up-front so renames during iteration do not
    // invalidate the directory iterator.
    let entries: Vec<_> = read_dir.flatten().collect();

    for entry in entries {
        let original_path = entry.path();
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        let lower = name.to_lowercase();

        // Compute the path we want to end up at after the (possible) rename.
        let final_path = if name.as_ref() == lower.as_str() {
            // Already lowercase — nothing to do.
            original_path
        } else {
            let target = dir.join(&lower);

            if target.exists() {
                match (original_path.is_dir(), target.is_dir()) {
                    (true, true) => {
                        if let Err(e) = merge_directory_into(&original_path, &target) {
                            log::warn!(
                                "[normalize] Failed to merge {:?} into {:?}: {e}",
                                original_path,
                                target,
                            );
                            original_path
                        } else {
                            target
                        }
                    }
                    _ => {
                        // Keep original on file/file or file/dir collisions to avoid
                        // unintended overwrite.
                        log::debug!(
                            "[normalize] Skipping {:?}: target {:?} already exists",
                            name,
                            lower,
                        );
                        original_path
                    }
                }
            } else {
                match std::fs::rename(&original_path, &target) {
                    Ok(()) => {
                        log::trace!("[normalize] {:?} → {:?}", name, lower);
                        target
                    }
                    Err(e) => {
                        log::warn!(
                            "[normalize] Failed to rename {:?} → {:?}: {e}",
                            original_path,
                            target,
                        );
                        original_path
                    }
                }
            }
        };

        // Recurse into (possibly renamed) subdirectories.
        if final_path.is_dir() {
            normalize_paths_to_lowercase(&final_path);
        }
    }
}

fn merge_directory_into(src: &Path, dst: &Path) -> Result<(), String> {
    let entries =
        std::fs::read_dir(src).map_err(|e| format!("Failed reading {}: {e}", src.display()))?;
    for entry in entries.flatten() {
        let src_child = entry.path();
        let file_name = entry.file_name();
        let dst_child = dst.join(file_name.to_string_lossy().to_lowercase());
        if src_child.is_dir() {
            if !dst_child.exists() {
                std::fs::rename(&src_child, &dst_child).map_err(|e| {
                    format!(
                        "Failed moving directory {} -> {}: {e}",
                        src_child.display(),
                        dst_child.display()
                    )
                })?;
            } else if dst_child.is_dir() {
                merge_directory_into(&src_child, &dst_child)?;
            }
            continue;
        }
        if !dst_child.exists() {
            std::fs::rename(&src_child, &dst_child).map_err(|e| {
                format!(
                    "Failed moving file {} -> {}: {e}",
                    src_child.display(),
                    dst_child.display()
                )
            })?;
        } else {
            log::debug!(
                "[normalize] Skipping file collision {} -> {}",
                src_child.display(),
                dst_child.display()
            );
        }
    }
    if src.exists() {
        std::fs::remove_dir(src)
            .map_err(|e| format!("Failed removing merged source dir {}: {e}", src.display()))?;
    }
    Ok(())
}

// ── ExtractedArchive ──────────────────────────────────────────────────────────

/// A fully-extracted archive held in a temporary directory.
///
/// The archive is decompressed exactly **once** — via
/// [`ExtractedArchive::from_archive_in`] or [`ExtractedArchive::from_archive`].
/// All subsequent operations (strategy detection, FOMOD parsing, image
/// loading, and final installation) work from the fast local filesystem
/// instead of re-reading the compressed stream.  This is critical for solid
/// 7z archives where every random-access read would otherwise require
/// decompressing from the beginning.
///
/// ## Cleanup
///
/// Call [`ExtractedArchive::cleanup`] as soon as the mod files have been moved
/// to their final location.  [`Drop`] calls `cleanup` as a safety net, but
/// doing so eagerly means the hidden temp folder in the mods directory is
/// removed before any other Arc references (e.g. a still-open FOMOD wizard
/// window) are dropped.  Both `cleanup` and `Drop` are safe to call multiple
/// times — the second call is always a no-op.
#[derive(Debug)]
pub struct ExtractedArchive {
    /// Immutable snapshot of the temp directory path used by [`Self::dir`].
    /// Remains valid (as a path string) even after the directory has been
    /// physically removed by [`Self::cleanup`].
    dir_path: PathBuf,

    /// The actual `PathBuf` to remove, protected by a `Mutex<Option<…>>`.
    /// `cleanup` takes the value out (setting it to `None`) and removes the
    /// directory; subsequent calls are no-ops.
    cleanup_dir: Mutex<Option<PathBuf>>,

    /// Every file/directory path relative to `dir_path`, in original case
    /// with forward slashes.  Junk top-level entries (`__MACOSX`, etc.) are
    /// excluded.
    entries: Vec<String>,
}

impl ExtractedArchive {
    /// Extract `archive_path` fully into a hidden temporary directory
    /// **inside `parent_dir`** (e.g. `game.mods_dir()`).
    ///
    /// Placing the temp dir on the same filesystem as the final mod
    /// destination means the installation step can **move** (rename) files
    /// instead of copying them, making large archives install instantly.
    ///
    /// The temp directory is named `.linkmm_tmp_<pid>_<timestamp>_<n>` and is
    /// automatically removed when [`Self::cleanup`] is called (or when this
    /// value is dropped as a fallback).
    ///
    /// Supports `.zip`, `.7z`, and `.rar` archives.  The `progress` callback
    /// is invoked periodically with `(bytes_written, total_bytes)` so callers
    /// can display a real percentage bar; when `total_bytes` is zero the
    /// caller should treat progress as indeterminate.
    pub fn from_archive_in(
        archive_path: &Path,
        parent_dir: &Path,
        progress: &dyn Fn(u64, u64) -> bool,
    ) -> Result<Self, String> {
        let archive_name = archive_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| archive_path.display().to_string());

        log::info!(
            "[ExtractedArchive] Extracting to mods-dir temp | archive={}, parent={}",
            archive_name,
            parent_dir.display(),
        );

        // Ensure the parent directory exists (the mods dir may not have been
        // created yet for a freshly configured game).
        std::fs::create_dir_all(parent_dir).map_err(|e| {
            format!(
                "Failed to create parent directory {}: {e}",
                parent_dir.display()
            )
        })?;

        let start = std::time::Instant::now();
        let dir = create_temp_extract_dir_in(parent_dir)?;

        let extract_result = if has_zip_extension(archive_path) {
            extract_zip_to(archive_path, &dir, "", progress)
        } else if has_rar_extension(archive_path) {
            extract_rar_archive(archive_path, &dir)
        } else {
            extract_7z_archive_to(archive_path, &dir, "", progress)
        };

        if let Err(e) = extract_result {
            // Clean up the temp dir on failure so we do not leave orphaned
            // hidden directories in the mods folder.
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e);
        }

        let mut entries = Vec::new();
        collect_extracted_entries(&dir, &dir, &mut entries);

        let elapsed = start.elapsed();
        log::info!(
            "[ExtractedArchive] Ready | archive={}, entries={}, elapsed={:.2}s, dir={}",
            archive_name,
            entries.len(),
            elapsed.as_secs_f64(),
            dir.display(),
        );

        Ok(Self {
            dir_path: dir.clone(),
            cleanup_dir: Mutex::new(Some(dir)),
            entries,
        })
    }

    /// Extract `archive_path` fully into a fresh temporary directory in the
    /// **system temp dir** (`std::env::temp_dir()`).
    ///
    /// Prefer [`Self::from_archive_in`] when the final installation
    /// destination is known, so that the extracted files land on the same
    /// filesystem and can be moved (renamed) instead of copied.
    ///
    /// Supports `.zip`, `.7z`, and `.rar` archives.  The `progress` callback
    /// is invoked periodically with `(bytes_written, total_bytes)`.
    #[allow(dead_code)]
    pub fn from_archive(
        archive_path: &Path,
        progress: &dyn Fn(u64, u64) -> bool,
    ) -> Result<Self, String> {
        let archive_name = archive_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| archive_path.display().to_string());

        log::info!(
            "[ExtractedArchive] Extracting to system temp | archive={}",
            archive_name,
        );

        let start = std::time::Instant::now();
        let dir = create_temp_extract_dir()?;

        let extract_result = if has_zip_extension(archive_path) {
            extract_zip_to(archive_path, &dir, "", progress)
        } else if has_rar_extension(archive_path) {
            extract_rar_archive(archive_path, &dir)
        } else {
            extract_7z_archive_to(archive_path, &dir, "", progress)
        };

        if let Err(e) = extract_result {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e);
        }

        let mut entries = Vec::new();
        collect_extracted_entries(&dir, &dir, &mut entries);

        let elapsed = start.elapsed();
        log::info!(
            "[ExtractedArchive] Ready | archive={}, entries={}, elapsed={:.2}s, dir={}",
            archive_name,
            entries.len(),
            elapsed.as_secs_f64(),
            dir.display(),
        );

        Ok(Self {
            dir_path: dir.clone(),
            cleanup_dir: Mutex::new(Some(dir)),
            entries,
        })
    }

    /// Remove the temporary directory immediately.
    ///
    /// Called by the installer as soon as mod files have been moved to their
    /// final location.  [`Drop`] calls this method again as a safety net;
    /// both are idempotent — the second call is always a no-op.
    pub fn cleanup(&self) {
        // Take the PathBuf out of the Option under the lock, then drop the
        // lock before doing any filesystem work to minimise contention.
        let dir = match self.cleanup_dir.lock() {
            Ok(mut guard) => guard.take(),
            Err(_) => return, // Mutex poisoned — nothing safe to do.
        };

        if let Some(dir) = dir {
            log::debug!(
                "[ExtractedArchive] Removing temp dir | dir={}",
                dir.display()
            );
            if let Err(e) = std::fs::remove_dir_all(&dir) {
                // `NotFound` is expected when all files were moved out and
                // the directory is already gone, or when an earlier cleanup
                // call already removed it.
                if e.kind() != std::io::ErrorKind::NotFound {
                    log::warn!(
                        "[ExtractedArchive] Failed to remove temp dir {}: {e}",
                        dir.display()
                    );
                }
            }
        }
    }

    /// The temporary directory containing all extracted files.
    pub fn dir(&self) -> &Path {
        &self.dir_path
    }

    /// Every path in the extracted archive, relative to [`Self::dir`],
    /// in original case with forward slashes.
    pub fn entries(&self) -> &[String] {
        &self.entries
    }
}

impl Drop for ExtractedArchive {
    fn drop(&mut self) {
        // `cleanup` is idempotent: if the installer already called it this
        // is a cheap no-op (the Mutex holds `None`).
        self.cleanup();
    }
}

/// Recursively collect all file and directory paths under `root`, skipping
/// junk top-level entries like `__MACOSX`.
fn collect_extracted_entries(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if rel_str.is_empty() {
            continue;
        }

        // Skip junk top-level entries.
        if dir == root {
            let top = rel_str.split('/').next().unwrap_or("").to_lowercase();
            if JUNK_TOPLEVEL_ENTRIES.contains(&top.as_str()) {
                continue;
            }
        }

        out.push(rel_str.to_string());
        if path.is_dir() {
            collect_extracted_entries(root, &path, out);
        }
    }
}
