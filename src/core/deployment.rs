// ══════════════════════════════════════════════════════════════════════════════
// Mod Deployment System - Link-Based File Management
// ══════════════════════════════════════════════════════════════════════════════
//
// This module handles deploying and undeploying mods using symbolic or hard links.
// Core principle: Game Data/ directory contains ONLY links, never copies of mod files.

use std::fs;
use std::path::Path;

use super::installer_new::{determine_link_type, LinkKind};
use crate::core::games::Game;
use crate::core::mods::Mod;

// ── Link Creation ─────────────────────────────────────────────────────────────

/// Create a link (symlink or hardlink) from source to destination.
///
/// The link type is determined automatically based on filesystem boundaries.
/// Creates parent directories as needed.
///
/// # Arguments
/// * `src` - Source file in mod storage
/// * `dest` - Destination path in game directory
///
/// # Returns
/// * `Ok(LinkKind)` - The type of link that was created
/// * `Err(String)` - Error message if link creation failed
#[cfg(unix)]
pub fn create_link(src: &Path, dest: &Path) -> Result<LinkKind, String> {
    use std::os::unix::fs::symlink;

    // Create parent directory if needed
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create parent directory for {}: {}", dest.display(), e))?;
    }

    // Handle existing files at destination
    if dest.exists() || dest.is_symlink() {
        if dest.is_symlink() {
            // Check if it's a broken symlink
            if !dest.exists() {
                // Broken symlink - remove it
                fs::remove_file(dest)
                    .map_err(|e| format!("Failed to remove broken symlink {}: {}", dest.display(), e))?;
            } else {
                // Valid symlink or file - check if it points to our source
                if let Ok(target) = fs::read_link(dest)
                    && target == src {
                        // Already linked correctly
                        return Ok(LinkKind::Symlink);
                    }
                // Points elsewhere - skip to avoid overwriting another mod's link
                return Err(format!("Destination {} already exists (conflict)", dest.display()));
            }
        } else if dest.is_file() {
            // Real file exists - don't overwrite (might be vanilla game file)
            return Err(format!("Real file exists at {} (not overwriting)", dest.display()));
        }
    }

    // Determine link type based on filesystem
    let link_kind = determine_link_type(src, dest.parent().unwrap_or(dest));

    // Create the appropriate link type
    match link_kind {
        LinkKind::Hardlink => {
            fs::hard_link(src, dest).map_err(|e| {
                format!("Failed to create hardlink {} -> {}: {}", dest.display(), src.display(), e)
            })?;
        }
        LinkKind::Symlink => {
            symlink(src, dest).map_err(|e| {
                format!("Failed to create symlink {} -> {}: {}", dest.display(), src.display(), e)
            })?;
        }
    }

    Ok(link_kind)
}

#[cfg(not(unix))]
pub fn create_link(src: &Path, dest: &Path) -> Result<LinkKind, String> {
    Err("Link-based deployment is only supported on Unix systems".to_string())
}

/// Remove a link if it points to the specified source.
///
/// Only removes links that point to our source file. Preserves:
/// - Real files (vanilla game content)
/// - Links to other mods' files
/// - Directories
///
/// # Returns
/// * `Ok(true)` - Link was removed
/// * `Ok(false)` - No link existed or it pointed elsewhere
/// * `Err(String)` - Error during removal
#[cfg(unix)]
pub fn remove_link_if_matches(src: &Path, dest: &Path) -> Result<bool, String> {
    if !dest.exists() && !dest.is_symlink() {
        return Ok(false); // Nothing to remove
    }

    if dest.is_symlink() {
        // Check if symlink points to our source
        if let Ok(target) = fs::read_link(dest)
            && target == src {
                fs::remove_file(dest)
                    .map_err(|e| format!("Failed to remove symlink {}: {}", dest.display(), e))?;
                return Ok(true);
            }
        return Ok(false); // Points elsewhere
    }

    // Check if it's a hardlink to our source
    if dest.is_file() && src.is_file() {
        use std::os::unix::fs::MetadataExt;

        let src_meta = fs::metadata(src)
            .map_err(|e| format!("Failed to read source metadata: {}", e))?;
        let dest_meta = fs::metadata(dest)
            .map_err(|e| format!("Failed to read dest metadata: {}", e))?;

        // Same inode and device = hardlink to same file
        if src_meta.dev() == dest_meta.dev() && src_meta.ino() == dest_meta.ino() {
            fs::remove_file(dest)
                .map_err(|e| format!("Failed to remove hardlink {}: {}", dest.display(), e))?;
            return Ok(true);
        }
    }

    Ok(false) // Not our link
}

#[cfg(not(unix))]
pub fn remove_link_if_matches(_src: &Path, _dest: &Path) -> Result<bool, String> {
    Ok(false)
}

// ── Directory Linking ─────────────────────────────────────────────────────────

/// Recursively link all files from source directory into destination.
///
/// Creates destination directories as needed. Only links files (leaves),
/// not directories themselves.
///
/// # Returns
/// * `Ok(usize)` - Number of links created
/// * `Err(String)` - Error message
pub fn link_directory_recursive(src_dir: &Path, dest_dir: &Path) -> Result<usize, String> {
    if !src_dir.is_dir() {
        return Err(format!("Source is not a directory: {}", src_dir.display()));
    }

    let mut link_count = 0;

    let entries = fs::read_dir(src_dir)
        .map_err(|e| format!("Failed to read directory {}: {}", src_dir.display(), e))?;

    for entry in entries.flatten() {
        let src_path = entry.path();
        let file_name = entry.file_name();
        let dest_path = dest_dir.join(&file_name);

        if src_path.is_dir() {
            // Recurse into subdirectories
            link_count += link_directory_recursive(&src_path, &dest_path)?;
        } else if src_path.is_file() {
            // Link files
            match create_link(&src_path, &dest_path) {
                Ok(_) => link_count += 1,
                Err(e) => {
                    // Log conflict but don't abort entire deployment
                    log::warn!("Failed to link {}: {}", dest_path.display(), e);
                }
            }
        }
    }

    Ok(link_count)
}

/// Recursively remove links from destination that point to files in source.
///
/// Also removes empty directories left behind after unlinking.
///
/// # Returns
/// * `Ok(usize)` - Number of links removed
/// * `Err(String)` - Error message
pub fn unlink_directory_recursive(src_dir: &Path, dest_dir: &Path) -> Result<usize, String> {
    if !src_dir.is_dir() {
        return Ok(0); // Nothing to unlink
    }

    if !dest_dir.is_dir() {
        return Ok(0); // Destination doesn't exist
    }

    let mut unlink_count = 0;

    let entries = fs::read_dir(src_dir)
        .map_err(|e| format!("Failed to read directory {}: {}", src_dir.display(), e))?;

    for entry in entries.flatten() {
        let src_path = entry.path();
        let file_name = entry.file_name();
        let dest_path = dest_dir.join(&file_name);

        if src_path.is_dir() {
            // Recurse into subdirectories
            unlink_count += unlink_directory_recursive(&src_path, &dest_path)?;
        } else if src_path.is_file() {
            // Remove link if it matches
            match remove_link_if_matches(&src_path, &dest_path) {
                Ok(true) => unlink_count += 1,
                Ok(false) => {} // Not our link
                Err(e) => log::warn!("Failed to remove link {}: {}", dest_path.display(), e),
            }
        }
    }

    // Try to remove empty destination directory
    // Ignore errors - directory might have other mods' files or vanilla content
    if let Err(e) = fs::remove_dir(dest_dir)
        && e.kind() != std::io::ErrorKind::DirectoryNotEmpty
            && e.kind() != std::io::ErrorKind::NotFound
        {
            log::debug!("Could not remove directory {}: {}", dest_dir.display(), e);
        }

    Ok(unlink_count)
}

// ── High-Level Deployment ─────────────────────────────────────────────────────

/// Deploy a mod by creating links from mod storage to game directory.
///
/// Handles both:
/// - mod_dir/Data/ → game_dir/Data/ (standard layout)
/// - mod_dir root files → game_dir root (DLLs, ENB configs, etc.)
///
/// Flattens nested Data/Data/ structures to prevent double-nesting.
pub fn deploy_mod(game: &Game, mod_entry: &Mod) -> Result<DeploymentReport, String> {
    let _span = crate::core::logger::span(
        "deploy_mod",
        &format!("mod={}", mod_entry.name),
    );
    let mut report = DeploymentReport::default();

    // Deploy Data/ folder contents
    let data_dir = mod_entry.source_path.join("Data");
    if data_dir.is_dir() {
        // Link Data/ contents with flattening
        let data_links = link_mod_data_with_flatten(&data_dir, &game.data_path)?;
        report.data_links_created = data_links;

        // Link root-level files (DLLs, SKSE, ENB, etc.) to game root
        let root_links = link_root_files(&mod_entry.source_path, &game.root_path)?;
        report.root_links_created = root_links;
    } else {
        // Legacy flat layout - link directly from mod root
        let links = link_directory_recursive(&mod_entry.source_path, &game.data_path)?;
        report.data_links_created = links;
    }

    log::info!(
        "Deployed mod '{}': {} data links, {} root links",
        mod_entry.name,
        report.data_links_created,
        report.root_links_created
    );

    Ok(report)
}

/// Undeploy a mod by removing its links from the game directory.
///
/// Only removes links that point to this mod's files. Preserves vanilla
/// content and other mods' files.
pub fn undeploy_mod(game: &Game, mod_entry: &Mod) -> Result<DeploymentReport, String> {
    let _span = crate::core::logger::span(
        "undeploy_mod",
        &format!("mod={}", mod_entry.name),
    );
    let mut report = DeploymentReport::default();

    let data_dir = mod_entry.source_path.join("Data");
    if data_dir.is_dir() {
        // Unlink Data/ contents
        let data_unlinks = unlink_mod_data_with_flatten(&data_dir, &game.data_path)?;
        report.data_links_removed = data_unlinks;

        // Unlink root-level files
        let root_unlinks = unlink_root_files(&mod_entry.source_path, &game.root_path)?;
        report.root_links_removed = root_unlinks;

        // Also check Data/ for misplaced root files (migration)
        let migrated = unlink_root_files(&mod_entry.source_path, &game.data_path)?;
        report.root_links_removed += migrated;
    } else {
        // Legacy flat layout
        let unlinks = unlink_directory_recursive(&mod_entry.source_path, &game.data_path)?;
        report.data_links_removed = unlinks;
    }

    log::info!(
        "Undeployed mod '{}': removed {} data links, {} root links",
        mod_entry.name,
        report.data_links_removed,
        report.root_links_removed
    );

    Ok(report)
}

// ── Data Folder Flattening ────────────────────────────────────────────────────

/// Link mod Data/ folder contents with automatic flattening of nested Data/.
///
/// If source contains Data/Data/, the nested Data/ is flattened into the
/// target Data/ directory. This handles FOMOD configs that incorrectly use
/// destination="Data" relative to game root.
fn link_mod_data_with_flatten(src_data: &Path, dest_data: &Path) -> Result<usize, String> {
    if !src_data.is_dir() {
        return Ok(0);
    }

    let mut link_count = 0;

    let entries = fs::read_dir(src_data)
        .map_err(|e| format!("Failed to read Data directory {}: {}", src_data.display(), e))?;

    for entry in entries.flatten() {
        let src_path = entry.path();
        let file_name = entry.file_name();

        // Check for nested Data/ subdirectory (case-insensitive)
        if src_path.is_dir() && file_name.as_encoded_bytes().eq_ignore_ascii_case(b"data") {
            // Flatten: recurse into nested Data/ at same destination level
            link_count += link_mod_data_with_flatten(&src_path, dest_data)?;
            continue;
        }

        let dest_path = dest_data.join(&file_name);

        if src_path.is_dir() {
            link_count += link_directory_recursive(&src_path, &dest_path)?;
        } else if src_path.is_file() {
            match create_link(&src_path, &dest_path) {
                Ok(_) => link_count += 1,
                Err(e) => log::warn!("Failed to link {}: {}", dest_path.display(), e),
            }
        }
    }

    Ok(link_count)
}

/// Unlink mod Data/ folder contents with flattening logic.
///
/// Mirrors link_mod_data_with_flatten for removal.
fn unlink_mod_data_with_flatten(src_data: &Path, dest_data: &Path) -> Result<usize, String> {
    if !src_data.is_dir() || !dest_data.is_dir() {
        return Ok(0);
    }

    let mut unlink_count = 0;

    let entries = fs::read_dir(src_data)
        .map_err(|e| format!("Failed to read Data directory {}: {}", src_data.display(), e))?;

    for entry in entries.flatten() {
        let src_path = entry.path();
        let file_name = entry.file_name();

        // Check for nested Data/ subdirectory
        if src_path.is_dir() && file_name.as_encoded_bytes().eq_ignore_ascii_case(b"data") {
            // Flatten: recurse into nested Data/ at same destination level
            unlink_count += unlink_mod_data_with_flatten(&src_path, dest_data)?;
            continue;
        }

        let dest_path = dest_data.join(&file_name);

        if src_path.is_dir() {
            unlink_count += unlink_directory_recursive(&src_path, &dest_path)?;
        } else if src_path.is_file() {
            match remove_link_if_matches(&src_path, &dest_path) {
                Ok(true) => unlink_count += 1,
                Ok(false) => {}
                Err(e) => log::warn!("Failed to remove link {}: {}", dest_path.display(), e),
            }
        }
    }

    Ok(unlink_count)
}

// ── Root File Linking ─────────────────────────────────────────────────────────

/// Link root-level mod files (DLLs, SKSE, ENB configs) to game root.
///
/// Skips the Data/ subdirectory - that's handled separately.
fn link_root_files(mod_root: &Path, game_root: &Path) -> Result<usize, String> {
    if !mod_root.is_dir() {
        return Ok(0);
    }

    let mut link_count = 0;

    let entries = fs::read_dir(mod_root)
        .map_err(|e| format!("Failed to read mod root {}: {}", mod_root.display(), e))?;

    for entry in entries.flatten() {
        let src_path = entry.path();
        let file_name = entry.file_name();

        // Skip Data/ directory
        if file_name.as_encoded_bytes().eq_ignore_ascii_case(b"data") {
            continue;
        }

        let dest_path = game_root.join(&file_name);

        if src_path.is_dir() {
            link_count += link_directory_recursive(&src_path, &dest_path)?;
        } else if src_path.is_file() {
            match create_link(&src_path, &dest_path) {
                Ok(_) => link_count += 1,
                Err(e) => log::warn!("Failed to link root file {}: {}", dest_path.display(), e),
            }
        }
    }

    Ok(link_count)
}

/// Unlink root-level mod files from game root.
///
/// Mirrors link_root_files for removal.
fn unlink_root_files(mod_root: &Path, game_root: &Path) -> Result<usize, String> {
    if !mod_root.is_dir() || !game_root.is_dir() {
        return Ok(0);
    }

    let mut unlink_count = 0;

    let entries = fs::read_dir(mod_root)
        .map_err(|e| format!("Failed to read mod root {}: {}", mod_root.display(), e))?;

    for entry in entries.flatten() {
        let src_path = entry.path();
        let file_name = entry.file_name();

        // Skip Data/ directory
        if file_name.as_encoded_bytes().eq_ignore_ascii_case(b"data") {
            continue;
        }

        let dest_path = game_root.join(&file_name);

        if src_path.is_dir() {
            unlink_count += unlink_directory_recursive(&src_path, &dest_path)?;
        } else if src_path.is_file() {
            match remove_link_if_matches(&src_path, &dest_path) {
                Ok(true) => unlink_count += 1,
                Ok(false) => {}
                Err(e) => log::warn!("Failed to remove root link {}: {}", dest_path.display(), e),
            }
        }
    }

    Ok(unlink_count)
}

// ── Cleanup Utilities ─────────────────────────────────────────────────────────

/// Remove all symlinks recursively from a directory tree.
///
/// Used for cleaning up legacy nested Data/Data/ structures.
/// Preserves real files and directories.
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

                // Try to remove directory if now empty
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

        // Remove the now-empty Data/Data/ directory
        let _ = fs::remove_dir(&legacy_nested);
    }
}

// ── Deployment Report ─────────────────────────────────────────────────────────

/// Report of deployment/undeployment operations
#[derive(Debug, Default)]
pub struct DeploymentReport {
    pub data_links_created: usize,
    pub root_links_created: usize,
    pub data_links_removed: usize,
    pub root_links_removed: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    #[cfg(unix)]
    #[test]
    fn test_create_and_remove_symlink() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("source.txt");
        let dest = temp.path().join("link.txt");

        // Create source file
        let mut file = File::create(&src).unwrap();
        file.write_all(b"test content").unwrap();

        // Create link (will be hardlink if same filesystem, symlink otherwise)
        let link_kind = create_link(&src, &dest).unwrap();

        // Verify link was created
        assert!(dest.exists());

        // Verify it's either a symlink or hardlink to the same file
        if link_kind == LinkKind::Symlink {
            assert!(dest.is_symlink());
            assert_eq!(fs::read_link(&dest).unwrap(), src);
        } else {
            // Hardlink - check same inode
            use std::os::unix::fs::MetadataExt;
            let src_meta = fs::metadata(&src).unwrap();
            let dest_meta = fs::metadata(&dest).unwrap();
            assert_eq!(src_meta.ino(), dest_meta.ino());
        }

        // Remove link
        let removed = remove_link_if_matches(&src, &dest).unwrap();
        assert!(removed);
        assert!(!dest.exists());
    }

    #[cfg(unix)]
    #[test]
    fn test_remove_link_preserves_other_links() {
        let temp = TempDir::new().unwrap();
        let src1 = temp.path().join("source1.txt");
        let src2 = temp.path().join("source2.txt");
        let dest = temp.path().join("link.txt");

        File::create(&src1).unwrap();
        File::create(&src2).unwrap();

        // Create link to src1
        create_link(&src1, &dest).unwrap();

        // Verify link exists
        assert!(dest.exists());

        // Try to remove as if it were src2's link
        let removed = remove_link_if_matches(&src2, &dest).unwrap();
        assert!(!removed); // Should NOT remove
        assert!(dest.exists()); // Link still exists
    }

    #[cfg(unix)]
    #[test]
    fn test_link_directory_recursive() {
        let temp = TempDir::new().unwrap();
        let src_dir = temp.path().join("src");
        let dest_dir = temp.path().join("dest");

        // Create source directory structure
        fs::create_dir_all(src_dir.join("subdir")).unwrap();
        File::create(src_dir.join("file1.txt")).unwrap();
        File::create(src_dir.join("subdir/file2.txt")).unwrap();

        // Link recursively
        let count = link_directory_recursive(&src_dir, &dest_dir).unwrap();
        assert_eq!(count, 2); // Two files linked

        // Check links exist (may be symlinks or hardlinks depending on filesystem)
        assert!(dest_dir.join("file1.txt").exists());
        assert!(dest_dir.join("subdir/file2.txt").exists());
    }
}
