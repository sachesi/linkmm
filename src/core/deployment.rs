// ══════════════════════════════════════════════════════════════════════════════
// Mod Deployment System - Link-Based File Management
// ══════════════════════════════════════════════════════════════════════════════
//
// This module handles deploying and undeploying mods using symbolic or hard links.
// Core principle: Game Data/ directory contains ONLY links, never copies of mod files.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::installer::{LinkKind, determine_link_type};
use crate::core::games::Game;
use crate::core::mods::{Mod, ModDatabase};
use crate::core::workspace;

const DEPLOY_STATE_FILE: &str = "deployment_state.toml";
const LEGACY_BACKUP_PREFIX: &str = "backups";

#[derive(Debug, Clone)]
pub struct DeploymentBackupStatus {
    pub backup_root: PathBuf,
    pub backup_entries: usize,
    pub existing_payload_files: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct DeploymentState {
    backups: HashMap<String, String>,
    deployed: HashMap<String, String>,
    owners: HashMap<String, String>,
}

impl DeploymentState {
    fn path(game: &Game, profile_id: &str) -> PathBuf {
        game.config_dir()
            .join("profiles")
            .join(profile_id)
            .join(DEPLOY_STATE_FILE)
    }

    fn load(game: &Game, profile_id: &str) -> Self {
        let path = Self::path(game, profile_id);
        let Ok(raw) = fs::read_to_string(path) else {
            return Self::default();
        };
        toml::from_str(&raw).unwrap_or_default()
    }

    fn save(&self, game: &Game, profile_id: &str) -> Result<(), String> {
        fs::create_dir_all(game.config_dir().join("profiles").join(profile_id))
            .map_err(|e| format!("Failed to create config dir for deployment state: {e}"))?;
        let raw = toml::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize deployment state: {e}"))?;
        fs::write(Self::path(game, profile_id), raw)
            .map_err(|e| format!("Failed to write deployment state: {e}"))?;
        Ok(())
    }
}

fn backup_payload_root(game: &Game, profile_id: &str) -> PathBuf {
    game.mods_dir()
        .join("profiles")
        .join(profile_id)
        .join("deployment_backups")
}

fn resolve_backup_payload_path(game: &Game, profile_id: &str, recorded: &str) -> PathBuf {
    let recorded_path = PathBuf::from(recorded);
    if recorded_path.is_absolute() {
        return recorded_path;
    }
    let legacy = game.config_dir().join(&recorded_path);
    if legacy.exists() {
        return legacy;
    }
    let normalized = if recorded.starts_with(LEGACY_BACKUP_PREFIX) {
        recorded_path
            .strip_prefix(LEGACY_BACKUP_PREFIX)
            .map(Path::to_path_buf)
            .unwrap_or(recorded_path)
    } else {
        recorded_path
    };
    backup_payload_root(game, profile_id).join(normalized)
}

fn backup_record_value(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

const ASSETS_DEPLOYER_ID: &str = "assets";

trait Deployer {
    fn id(&self) -> &'static str;
    fn rebuild(&self, game: &Game, db: &mut ModDatabase) -> Result<(), String>;
}

struct AssetsDeployer;
struct PluginsDeployer;

#[derive(Debug, Clone)]
struct DesiredDeployment {
    source: PathBuf,
    owner_id: String,
}

#[derive(Debug, Clone, Default)]
pub struct DeploymentPreview {
    pub links_to_create: Vec<PathBuf>,
    pub links_to_replace: Vec<PathBuf>,
    pub links_to_remove: Vec<PathBuf>,
    pub real_files_to_backup: Vec<PathBuf>,
    pub backups_to_restore: Vec<PathBuf>,
    pub backups_remaining: Vec<PathBuf>,
    pub generated_outputs_participating: Vec<String>,
    pub blocked_paths: Vec<String>,
}

impl DeploymentPreview {
    pub fn summary_line(&self) -> String {
        format!(
            "Add: {} · Replace: {} · Remove: {} · Backup: {} · Restore: {}",
            self.links_to_create.len(),
            self.links_to_replace.len(),
            self.links_to_remove.len(),
            self.real_files_to_backup.len(),
            self.backups_to_restore.len()
        )
    }
}

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
        fs::create_dir_all(parent).map_err(|e| {
            format!(
                "Failed to create parent directory for {}: {}",
                dest.display(),
                e
            )
        })?;
    }

    // Handle existing files at destination
    if dest.exists() || dest.is_symlink() {
        if dest.is_symlink() {
            // Check if it's a broken symlink
            if !dest.exists() {
                // Broken symlink - remove it
                fs::remove_file(dest).map_err(|e| {
                    format!("Failed to remove broken symlink {}: {}", dest.display(), e)
                })?;
            } else {
                // Valid symlink or file - check if it points to our source
                if let Ok(target) = fs::read_link(dest)
                    && target == src
                {
                    // Already linked correctly
                    return Ok(LinkKind::Symlink);
                }
                // Points elsewhere - skip to avoid overwriting another mod's link
                return Err(format!(
                    "Destination {} already linked by higher-priority mod",
                    dest.display()
                ));
            }
        } else if dest.is_file() {
            // Real file exists - don't overwrite (might be vanilla game file)
            return Err(format!(
                "Real file exists at {} (priority conflict: not overwriting real file)",
                dest.display()
            ));
        }
    }

    // Determine link type based on filesystem
    let link_kind = determine_link_type(src, dest.parent().unwrap_or(dest));

    // Create the appropriate link type
    match link_kind {
        LinkKind::Hardlink => {
            fs::hard_link(src, dest).map_err(|e| {
                format!(
                    "Failed to create hardlink {} -> {}: {}",
                    dest.display(),
                    src.display(),
                    e
                )
            })?;
        }
        LinkKind::Symlink => {
            symlink(src, dest).map_err(|e| {
                format!(
                    "Failed to create symlink {} -> {}: {}",
                    dest.display(),
                    src.display(),
                    e
                )
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
            && target == src
        {
            fs::remove_file(dest)
                .map_err(|e| format!("Failed to remove symlink {}: {}", dest.display(), e))?;
            return Ok(true);
        }
        return Ok(false); // Points elsewhere
    }

    // Check if it's a hardlink to our source
    if dest.is_file() && src.is_file() {
        use std::os::unix::fs::MetadataExt;

        let src_meta =
            fs::metadata(src).map_err(|e| format!("Failed to read source metadata: {}", e))?;
        let dest_meta =
            fs::metadata(dest).map_err(|e| format!("Failed to read dest metadata: {}", e))?;

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
/// * `Ok((linked, skipped))` - Number of links created and skipped (priority conflicts)
/// * `Err(String)` - Error message
pub fn link_directory_recursive(src_dir: &Path, dest_dir: &Path) -> Result<(usize, usize), String> {
    if !src_dir.is_dir() {
        return Err(format!("Source is not a directory: {}", src_dir.display()));
    }

    let mut link_count = 0;
    let mut skip_count = 0;

    let entries = fs::read_dir(src_dir)
        .map_err(|e| format!("Failed to read directory {}: {}", src_dir.display(), e))?;

    for entry in entries.flatten() {
        let src_path = entry.path();
        let file_name = entry.file_name();
        let dest_path = dest_dir.join(&file_name);

        if src_path.is_dir() {
            // Recurse into subdirectories
            let (linked, skipped) = link_directory_recursive(&src_path, &dest_path)?;
            link_count += linked;
            skip_count += skipped;
        } else if src_path.is_file() {
            // Link files
            match create_link(&src_path, &dest_path) {
                Ok(_) => link_count += 1,
                Err(e) => {
                    // Expected during priority deployment — higher-priority mod already linked
                    log::debug!("Skipped (conflict): {} — {}", dest_path.display(), e);
                    skip_count += 1;
                }
            }
        }
    }

    Ok((link_count, skip_count))
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

pub fn rebuild_deployment(game: &Game, db: &mut ModDatabase) -> Result<(), String> {
    workspace::mark_operation(
        &game.id,
        &db.active_profile_id,
        workspace::WorkspaceOperation::Deploy,
    );
    let deployers: Vec<Box<dyn Deployer>> =
        vec![Box::new(AssetsDeployer), Box::new(PluginsDeployer)];
    for deployer in deployers {
        log::debug!("[Deployer:{}] Rebuilding", deployer.id());
        if let Err(e) = deployer.rebuild(game, db) {
            workspace::mark_deploy_failure(
                &game.id,
                &db.active_profile_id,
                format!("Deployment failed: {e}"),
            );
            return Err(e);
        }
    }
    workspace::mark_deployed_clean(game, db)?;
    let backup_status = deployment_backup_status(game, &db.active_profile_id)?;
    workspace::set_status(
        &game.id,
        &db.active_profile_id,
        workspace::StatusSeverity::Info,
        if backup_status.backup_entries > 0 {
            format!(
                "Deployment is up to date; preserved {} original file(s) in backup storage",
                backup_status.backup_entries
            )
        } else {
            "Deployment is up to date; no preserved original files pending restore".to_string()
        },
    );
    Ok(())
}

pub fn deployment_backup_status(
    game: &Game,
    profile_id: &str,
) -> Result<DeploymentBackupStatus, String> {
    let state = DeploymentState::load(game, profile_id);
    let backup_root = backup_payload_root(game, profile_id);
    let mut existing_payload_files = 0usize;
    for recorded in state.backups.values() {
        let path = resolve_backup_payload_path(game, profile_id, recorded);
        if path.is_file() {
            existing_payload_files += 1;
        }
    }
    Ok(DeploymentBackupStatus {
        backup_root,
        backup_entries: state.backups.len(),
        existing_payload_files,
    })
}

pub fn cleanup_stale_backup_payloads(game: &Game, profile_id: &str) -> Result<usize, String> {
    let state = DeploymentState::load(game, profile_id);
    let live: HashSet<PathBuf> = state
        .backups
        .values()
        .map(|recorded| resolve_backup_payload_path(game, profile_id, recorded))
        .collect();
    let root = backup_payload_root(game, profile_id);
    if !root.exists() {
        return Ok(0);
    }
    let mut removed = 0usize;
    cleanup_stale_recursive(&root, &live, &mut removed)?;
    Ok(removed)
}

impl Deployer for AssetsDeployer {
    fn id(&self) -> &'static str {
        ASSETS_DEPLOYER_ID
    }

    fn rebuild(&self, game: &Game, db: &mut ModDatabase) -> Result<(), String> {
        let desired = build_assets_plan(db, self.id());
        apply_assets_plan(game, &db.active_profile_id, desired)
    }
}

impl Deployer for PluginsDeployer {
    fn id(&self) -> &'static str {
        "plugins"
    }

    fn rebuild(&self, game: &Game, db: &mut ModDatabase) -> Result<(), String> {
        db.write_plugins_txt(game)
    }
}

fn build_assets_plan(db: &ModDatabase, deployer_id: &str) -> HashMap<PathBuf, DesiredDeployment> {
    let mut desired = HashMap::new();
    for mod_entry in db
        .mods
        .iter()
        .filter(|m| m.enabled && m.deployer.as_str() == deployer_id)
    {
        for (dest, src) in collect_mod_destinations(mod_entry) {
            desired.insert(
                dest,
                DesiredDeployment {
                    source: src,
                    owner_id: format!("mod:{}", mod_entry.id),
                },
            );
        }
    }
    for output in db.generated_outputs.iter().filter(|o| {
        o.enabled
            && o.deployer.as_str() == deployer_id
            && o.manager_profile_id == db.active_profile_id
    }) {
        for (dest, src) in collect_generated_output_destinations(output) {
            desired.insert(
                dest,
                DesiredDeployment {
                    source: src,
                    owner_id: format!("generated:{}", output.id),
                },
            );
        }
    }
    desired
}

fn apply_assets_plan(
    game: &Game,
    profile_id: &str,
    desired: HashMap<PathBuf, DesiredDeployment>,
) -> Result<(), String> {
    let mut state = DeploymentState::load(game, profile_id);
    let desired_set: HashSet<PathBuf> = desired.keys().cloned().collect();

    for (dest_rel, src_rel) in state.deployed.clone() {
        let dest = game.root_path.join(&dest_rel);
        let src = PathBuf::from(&src_rel);
        if !desired_set.contains(&PathBuf::from(&dest_rel))
            || desired
                .get(&PathBuf::from(&dest_rel))
                .is_none_or(|s| s.source != src)
        {
            let removed = remove_link_if_matches(&src, &dest).unwrap_or(false);
            if !removed && (dest.exists() || dest.is_symlink()) {
                let _ = fs::remove_file(&dest);
            }
            state.deployed.remove(&dest_rel);
            state.owners.remove(&dest_rel);
        }
    }

    for (dest_rel, entry) in &desired {
        let dest = game.root_path.join(dest_rel);
        ensure_path_ready_for_link(game, profile_id, &mut state, &dest)?;
        if remove_link_if_matches(&entry.source, &dest).is_err()
            && (dest.exists() || dest.is_symlink())
        {
            let _ = fs::remove_file(&dest);
        }
        let _ = create_link(&entry.source, &dest)?;
        state.deployed.insert(
            dest_rel.to_string_lossy().into_owned(),
            entry.source.to_string_lossy().into_owned(),
        );
        state.owners.insert(
            dest_rel.to_string_lossy().into_owned(),
            entry.owner_id.clone(),
        );
    }

    restore_backups_not_in_desired(game, profile_id, &mut state, &desired_set)?;
    state.save(game, profile_id)?;
    let _ = cleanup_stale_backup_payloads(game, profile_id);
    Ok(())
}

fn restore_backups_not_in_desired(
    game: &Game,
    profile_id: &str,
    state: &mut DeploymentState,
    desired: &HashSet<PathBuf>,
) -> Result<(), String> {
    let backup_keys: Vec<String> = state.backups.keys().cloned().collect();
    for dest_rel in backup_keys {
        if desired.contains(&PathBuf::from(&dest_rel)) {
            continue;
        }
        let dest = game.root_path.join(&dest_rel);
        if dest.exists() || dest.is_symlink() {
            let _ = fs::remove_file(&dest);
        }
        if let Some(backup_rel) = state.backups.remove(&dest_rel) {
            let backup_path = resolve_backup_payload_path(game, profile_id, &backup_rel);
            if backup_path.exists() {
                move_file_with_cross_fs_fallback(
                    &backup_path,
                    &dest,
                    &format!("restore backup for {}", dest.display()),
                )?;
                let stop = backup_payload_root(game, profile_id);
                cleanup_empty_parents(&backup_path, &stop);
            }
        }
    }
    Ok(())
}

fn ensure_path_ready_for_link(
    game: &Game,
    profile_id: &str,
    state: &mut DeploymentState,
    dest: &Path,
) -> Result<(), String> {
    if dest.is_symlink() {
        return Ok(());
    }
    if !dest.exists() {
        return Ok(());
    }
    if !dest.is_file() {
        return Err(format!(
            "Destination exists and is not a file: {}",
            dest.display()
        ));
    }

    let rel = dest
        .strip_prefix(&game.root_path)
        .map_err(|_| format!("Destination not inside game root: {}", dest.display()))?
        .to_path_buf();
    let rel_s = rel.to_string_lossy().into_owned();
    if state.backups.contains_key(&rel_s) {
        fs::remove_file(dest)
            .map_err(|e| format!("Failed to remove existing file {}: {e}", dest.display()))?;
        return Ok(());
    }
    let backup_path = backup_payload_root(game, &profile_id).join(&rel);
    if let Some(parent) = backup_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create backup directory: {e}"))?;
    }
    move_file_with_cross_fs_fallback(
        dest,
        &backup_path,
        &format!("backup existing file {}", dest.display()),
    )?;
    state
        .backups
        .insert(rel_s, backup_record_value(&backup_path));
    Ok(())
}

fn move_file_with_cross_fs_fallback(src: &Path, dest: &Path, action: &str) -> Result<(), String> {
    match fs::rename(src, dest) {
        Ok(()) => Ok(()),
        Err(err) if is_cross_device_link_error(&err) => {
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    format!("Failed to create destination directory while trying to {action}: {e}")
                })?;
            }

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

fn cleanup_empty_parents(path: &Path, stop_at: &Path) {
    let mut current = path.parent();
    while let Some(dir) = current {
        if !dir.starts_with(stop_at) || dir == stop_at {
            break;
        }
        match fs::remove_dir(dir) {
            Ok(()) => current = dir.parent(),
            Err(_) => break,
        }
    }
}

fn cleanup_stale_recursive(
    dir: &Path,
    live: &HashSet<PathBuf>,
    removed: &mut usize,
) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("Failed reading {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("Failed reading entry in {}: {e}", dir.display()))?;
        let path = entry.path();
        if path.is_dir() {
            cleanup_stale_recursive(&path, live, removed)?;
            let _ = fs::remove_dir(&path);
        } else if path.is_file() && !live.contains(&path) {
            fs::remove_file(&path)
                .map_err(|e| format!("Failed removing stale backup {}: {e}", path.display()))?;
            *removed += 1;
        }
    }
    Ok(())
}

fn is_cross_device_link_error(err: &io::Error) -> bool {
    err.raw_os_error() == Some(18)
}

fn collect_mod_destinations(mod_entry: &Mod) -> HashMap<PathBuf, PathBuf> {
    let mut map = HashMap::new();
    let data_dir = mod_entry.source_path.join("Data");
    if data_dir.is_dir() {
        collect_data_paths_recursive(&data_dir, Path::new("Data"), &mut map);
        collect_root_paths_recursive(&mod_entry.source_path, Path::new(""), &mut map);
    } else {
        collect_data_paths_recursive(&mod_entry.source_path, Path::new("Data"), &mut map);
    }
    map
}

fn collect_generated_output_destinations(
    output: &crate::core::mods::GeneratedOutputPackage,
) -> HashMap<PathBuf, PathBuf> {
    let mut map = HashMap::new();
    collect_data_paths_recursive(&output.source_path, Path::new("Data"), &mut map);
    map
}

fn collect_data_paths_recursive(src: &Path, dest: &Path, out: &mut HashMap<PathBuf, PathBuf>) {
    let Ok(entries) = fs::read_dir(src) else {
        return;
    };
    for entry in entries.flatten() {
        let src_path = entry.path();
        let name = entry.file_name();
        if src_path.is_dir() && name.as_encoded_bytes().eq_ignore_ascii_case(b"data") {
            collect_data_paths_recursive(&src_path, dest, out);
        } else if src_path.is_dir() {
            collect_data_paths_recursive(&src_path, &dest.join(&name), out);
        } else if src_path.is_file() {
            out.insert(dest.join(&name), src_path);
        }
    }
}

fn collect_root_paths_recursive(src: &Path, dest: &Path, out: &mut HashMap<PathBuf, PathBuf>) {
    let Ok(entries) = fs::read_dir(src) else {
        return;
    };
    for entry in entries.flatten() {
        let src_path = entry.path();
        let name = entry.file_name();
        if name.as_encoded_bytes().eq_ignore_ascii_case(b"data") {
            continue;
        }
        if src_path.is_dir() {
            collect_root_paths_recursive(&src_path, &dest.join(&name), out);
        } else if src_path.is_file() {
            out.insert(dest.join(&name), src_path);
        }
    }
}

/// Deploy a mod by creating links from mod storage to game directory.
///
/// Handles both:
/// - mod_dir/Data/ → game_dir/Data/ (standard layout)
/// - mod_dir root files → game_dir root (DLLs, ENB configs, etc.)
///
/// Flattens nested Data/Data/ structures to prevent double-nesting.
pub fn deploy_mod(game: &Game, mod_entry: &Mod) -> Result<DeploymentReport, String> {
    let _span = crate::core::logger::span("deploy_mod", &format!("mod={}", mod_entry.name));
    let mut report = DeploymentReport::default();

    // Deploy Data/ folder contents
    let data_dir = mod_entry.source_path.join("Data");
    if data_dir.is_dir() {
        // Link Data/ contents with flattening
        let (data_linked, data_skipped) = link_mod_data_with_flatten(&data_dir, &game.data_path)?;
        report.data_links_created = data_linked;
        report.data_links_skipped = data_skipped;

        // Link root-level files (DLLs, SKSE, ENB, etc.) to game root
        let (root_linked, root_skipped) = link_root_files(&mod_entry.source_path, &game.root_path)?;
        report.root_links_created = root_linked;
        report.root_links_skipped = root_skipped;
    } else {
        // Legacy flat layout - link directly from mod root
        let (linked, skipped) = link_directory_recursive(&mod_entry.source_path, &game.data_path)?;
        report.data_links_created = linked;
        report.data_links_skipped = skipped;
    }

    let total_skipped = report.data_links_skipped + report.root_links_skipped;
    if total_skipped > 0 {
        log::info!(
            "Deployed mod '{}': {} data links, {} root links ({} skipped due to priority)",
            mod_entry.name,
            report.data_links_created,
            report.root_links_created,
            total_skipped
        );
    } else {
        log::info!(
            "Deployed mod '{}': {} data links, {} root links",
            mod_entry.name,
            report.data_links_created,
            report.root_links_created
        );
    }

    Ok(report)
}

/// Undeploy a mod by removing its links from the game directory.
///
/// Only removes links that point to this mod's files. Preserves vanilla
/// content and other mods' files.
pub fn undeploy_mod(game: &Game, mod_entry: &Mod) -> Result<DeploymentReport, String> {
    let _span = crate::core::logger::span("undeploy_mod", &format!("mod={}", mod_entry.name));
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
fn link_mod_data_with_flatten(src_data: &Path, dest_data: &Path) -> Result<(usize, usize), String> {
    if !src_data.is_dir() {
        return Ok((0, 0));
    }

    let mut link_count = 0;
    let mut skip_count = 0;

    let entries = fs::read_dir(src_data).map_err(|e| {
        format!(
            "Failed to read Data directory {}: {}",
            src_data.display(),
            e
        )
    })?;

    for entry in entries.flatten() {
        let src_path = entry.path();
        let file_name = entry.file_name();

        // Check for nested Data/ subdirectory (case-insensitive)
        if src_path.is_dir() && file_name.as_encoded_bytes().eq_ignore_ascii_case(b"data") {
            // Flatten: recurse into nested Data/ at same destination level
            let (linked, skipped) = link_mod_data_with_flatten(&src_path, dest_data)?;
            link_count += linked;
            skip_count += skipped;
            continue;
        }

        let dest_path = dest_data.join(&file_name);

        if src_path.is_dir() {
            let (linked, skipped) = link_directory_recursive(&src_path, &dest_path)?;
            link_count += linked;
            skip_count += skipped;
        } else if src_path.is_file() {
            match create_link(&src_path, &dest_path) {
                Ok(_) => link_count += 1,
                Err(e) => {
                    log::debug!("Skipped (conflict): {} — {}", dest_path.display(), e);
                    skip_count += 1;
                }
            }
        }
    }

    Ok((link_count, skip_count))
}

/// Unlink mod Data/ folder contents with flattening logic.
///
/// Mirrors link_mod_data_with_flatten for removal.
fn unlink_mod_data_with_flatten(src_data: &Path, dest_data: &Path) -> Result<usize, String> {
    if !src_data.is_dir() || !dest_data.is_dir() {
        return Ok(0);
    }

    let mut unlink_count = 0;

    let entries = fs::read_dir(src_data).map_err(|e| {
        format!(
            "Failed to read Data directory {}: {}",
            src_data.display(),
            e
        )
    })?;

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
fn link_root_files(mod_root: &Path, game_root: &Path) -> Result<(usize, usize), String> {
    if !mod_root.is_dir() {
        return Ok((0, 0));
    }

    let mut link_count = 0;
    let mut skip_count = 0;

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
            let (linked, skipped) = link_directory_recursive(&src_path, &dest_path)?;
            link_count += linked;
            skip_count += skipped;
        } else if src_path.is_file() {
            match create_link(&src_path, &dest_path) {
                Ok(_) => link_count += 1,
                Err(e) => {
                    log::debug!("Skipped (conflict): {} — {}", dest_path.display(), e);
                    skip_count += 1;
                }
            }
        }
    }

    Ok((link_count, skip_count))
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
    pub data_links_skipped: usize,
    pub root_links_skipped: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::games::{Game, GameKind, GameLauncherSource, UmuGameConfig};
    use crate::core::mods::{Mod, ModDatabase};
    use crate::core::workspace;
    use std::fs::File;
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
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
        let (count, skipped) = link_directory_recursive(&src_dir, &dest_dir).unwrap();
        assert_eq!(count, 2); // Two files linked
        assert_eq!(skipped, 0); // No conflicts

        // Check links exist (may be symlinks or hardlinks depending on filesystem)
        assert!(dest_dir.join("file1.txt").exists());
        assert!(dest_dir.join("subdir/file2.txt").exists());
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

    fn test_game(layout: &TempDir) -> Game {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = format!("deploy_test_{}", COUNTER.fetch_add(1, Ordering::Relaxed));
        let root = layout.path().join("game_root");
        let data = root.join("Data");
        let mods_base = layout.path().join("mods_base");
        let prefix = layout.path().join("umu_prefix");
        let plugins_dir = prefix
            .join("pfx")
            .join("drive_c")
            .join("users")
            .join("steamuser")
            .join("AppData")
            .join("Local")
            .join(GameKind::SkyrimSE.local_app_data_folder());
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(mods_base.join("mods").join(&id)).unwrap();
        fs::create_dir_all(&plugins_dir).unwrap();

        Game {
            id,
            name: "Test".to_string(),
            kind: GameKind::SkyrimSE,
            launcher_source: GameLauncherSource::NonSteamUmu,
            steam_app_id: None,
            root_path: root,
            data_path: data,
            mods_base_dir: Some(mods_base),
            umu_config: Some(UmuGameConfig {
                exe_path: PathBuf::from("game.exe"),
                prefix_path: Some(prefix),
                proton_path: None,
            }),
        }
    }

    fn add_mod(
        game: &Game,
        db: &mut ModDatabase,
        name: &str,
        rel: &str,
        content: &[u8],
        enabled: bool,
    ) {
        let mod_dir = game.mods_dir().join(name);
        fs::create_dir_all(mod_dir.join("Data")).unwrap();
        let path = mod_dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, content).unwrap();
        let mut m = Mod::new(name, mod_dir);
        m.enabled = enabled;
        db.mods.push(m);
    }

    #[cfg(unix)]
    #[test]
    fn rebuild_uses_order_for_conflict_resolution() {
        let temp = TempDir::new().unwrap();
        let game = test_game(&temp);
        let mut db = ModDatabase::default();
        add_mod(
            &game,
            &mut db,
            "a",
            "Data/textures/file.txt",
            b"from-a",
            true,
        );
        add_mod(
            &game,
            &mut db,
            "b",
            "Data/textures/file.txt",
            b"from-b",
            true,
        );

        rebuild_deployment(&game, &mut db).unwrap();
        let target_first = fs::read(game.data_path.join("textures/file.txt")).unwrap();
        assert_eq!(target_first, b"from-b");

        db.mods.swap(0, 1);
        rebuild_deployment(&game, &mut db).unwrap();
        let target_second = fs::read(game.data_path.join("textures/file.txt")).unwrap();
        assert_eq!(target_second, b"from-a");
    }

    #[cfg(unix)]
    #[test]
    fn rebuild_depends_on_current_state_not_toggle_history() {
        let temp = TempDir::new().unwrap();
        let game = test_game(&temp);
        let mut db = ModDatabase::default();
        add_mod(&game, &mut db, "a", "Data/meshes/conflict.nif", b"a", true);
        add_mod(&game, &mut db, "b", "Data/meshes/conflict.nif", b"b", false);

        rebuild_deployment(&game, &mut db).unwrap();
        let target = fs::read(game.data_path.join("meshes/conflict.nif")).unwrap();
        assert_eq!(target, b"a");

        db.mods[1].enabled = true;
        rebuild_deployment(&game, &mut db).unwrap();
        db.mods[1].enabled = false;
        rebuild_deployment(&game, &mut db).unwrap();

        let final_target = fs::read(game.data_path.join("meshes/conflict.nif")).unwrap();
        assert_eq!(final_target, b"a");
    }

    #[cfg(unix)]
    #[test]
    fn rebuild_backs_up_and_restores_real_files() {
        let temp = TempDir::new().unwrap();
        let game = test_game(&temp);
        let target = game.data_path.join("textures/original.dds");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, b"vanilla").unwrap();

        let mut db = ModDatabase::default();
        add_mod(
            &game,
            &mut db,
            "override",
            "Data/textures/original.dds",
            b"modded",
            true,
        );
        rebuild_deployment(&game, &mut db).unwrap();

        assert!(target.exists());
        let backup = game
            .mods_dir()
            .join("profiles")
            .join(&db.active_profile_id)
            .join("deployment_backups")
            .join("Data")
            .join("textures")
            .join("original.dds");
        assert!(backup.exists());

        db.mods[0].enabled = false;
        rebuild_deployment(&game, &mut db).unwrap();
        let restored = fs::read(&target).unwrap();
        assert_eq!(restored, b"vanilla");
        assert!(!backup.exists());
    }

    #[cfg(unix)]
    #[test]
    fn restore_works_with_legacy_config_backup_record() {
        let temp = TempDir::new().unwrap();
        let game = test_game(&temp);
        let mut db = ModDatabase::default();
        let target = game.data_path.join("textures/legacy.dds");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, b"legacy").unwrap();
        let legacy_backup = game
            .config_dir()
            .join("backups")
            .join("Data")
            .join("textures")
            .join("legacy.dds");
        fs::create_dir_all(legacy_backup.parent().unwrap()).unwrap();
        fs::rename(&target, &legacy_backup).unwrap();

        let state = DeploymentState {
            backups: HashMap::from([(
                "Data/textures/legacy.dds".to_string(),
                "backups/Data/textures/legacy.dds".to_string(),
            )]),
            deployed: HashMap::new(),
            owners: HashMap::new(),
        };
        state.save(&game, &db.active_profile_id).unwrap();
        rebuild_deployment(&game, &mut db).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"legacy");
        assert!(!legacy_backup.exists());
    }

    #[cfg(unix)]
    #[test]
    fn backup_payloads_are_profile_isolated() {
        let temp = TempDir::new().unwrap();
        let game = test_game(&temp);
        let target = game.data_path.join("textures/profile.dds");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, b"original").unwrap();
        let mut db = ModDatabase::default();
        add_mod(
            &game,
            &mut db,
            "override",
            "Data/textures/profile.dds",
            b"new",
            true,
        );
        rebuild_deployment(&game, &mut db).unwrap();
        let default_backup =
            backup_payload_root(&game, &db.active_profile_id).join("Data/textures/profile.dds");
        assert!(default_backup.exists());

        db.switch_active_profile("second");
        db.mods[0].enabled = false;
        rebuild_deployment(&game, &mut db).unwrap();
        fs::write(&target, b"second-profile").unwrap();
        db.mods[0].enabled = true;
        rebuild_deployment(&game, &mut db).unwrap();
        let second_backup = backup_payload_root(&game, "second").join("Data/textures/profile.dds");
        assert!(second_backup.exists());
        assert_ne!(default_backup, second_backup);
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_stale_backups_removes_unreferenced_payloads() {
        let temp = TempDir::new().unwrap();
        let game = test_game(&temp);
        let profile = "default";
        let root = backup_payload_root(&game, profile);
        let stale = root.join("Data/stale.txt");
        fs::create_dir_all(stale.parent().unwrap()).unwrap();
        fs::write(&stale, b"x").unwrap();
        let removed = cleanup_stale_backup_payloads(&game, profile).unwrap();
        assert_eq!(removed, 1);
        assert!(!stale.exists());
    }

    #[cfg(unix)]
    #[test]
    fn plugins_deployer_writes_plugins_txt_deterministically() {
        let temp = TempDir::new().unwrap();
        let game = test_game(&temp);
        let mut db = ModDatabase::default();
        add_mod(&game, &mut db, "base", "Data/Base.esm", b"m", true);
        add_mod(&game, &mut db, "addon", "Data/Addon.esp", b"p", true);
        db.plugin_load_order = vec!["Addon.esp".to_string(), "Base.esm".to_string()];
        db.plugin_disabled.insert("Addon.esp".to_string());

        rebuild_deployment(&game, &mut db).unwrap();
        let plugins_txt = fs::read_to_string(game.plugins_txt_path().unwrap()).unwrap();
        assert!(plugins_txt.contains("*Base.esm"));
        assert!(plugins_txt.contains("Addon.esp"));
    }

    #[cfg(unix)]
    #[test]
    fn generated_outputs_participate_in_conflict_resolution_order() {
        let temp = TempDir::new().unwrap();
        let game = test_game(&temp);
        let mut db = ModDatabase::default();
        add_mod(
            &game,
            &mut db,
            "base_mod",
            "Data/textures/shared.dds",
            b"mod",
            true,
        );

        let generated_dir = game.mods_dir().join("generated_outputs").join("bodyslide");
        fs::create_dir_all(generated_dir.join("textures")).unwrap();
        fs::write(generated_dir.join("textures/shared.dds"), b"generated").unwrap();
        let mut package = crate::core::mods::GeneratedOutputPackage::new(
            "BodySlide Output",
            "bodyslide",
            "default",
            db.active_profile_id.clone(),
            generated_dir,
        );
        package.enabled = true;
        db.generated_outputs.push(package);

        rebuild_deployment(&game, &mut db).unwrap();
        let winner = fs::read(game.data_path.join("textures/shared.dds")).unwrap();
        assert_eq!(winner, b"generated");

        db.generated_outputs[0].enabled = false;
        rebuild_deployment(&game, &mut db).unwrap();
        let winner_after = fs::read(game.data_path.join("textures/shared.dds")).unwrap();
        assert_eq!(winner_after, b"mod");
    }

    #[cfg(unix)]
    #[test]
    fn profile_switch_rebuilds_isolated_mod_state() {
        let temp = TempDir::new().unwrap();
        let game = test_game(&temp);
        let mut db = ModDatabase::default();
        add_mod(&game, &mut db, "a", "Data/textures/same.dds", b"a", true);
        add_mod(&game, &mut db, "b", "Data/textures/same.dds", b"b", false);
        db.save(&game);

        rebuild_deployment(&game, &mut db).unwrap();
        let winner_default = fs::read(game.data_path.join("textures/same.dds")).unwrap();
        assert_eq!(winner_default, b"a");

        db.switch_active_profile("second");
        db.mods[0].enabled = false;
        db.mods[1].enabled = true;
        rebuild_deployment(&game, &mut db).unwrap();
        db.save(&game);
        let winner_second = fs::read(game.data_path.join("textures/same.dds")).unwrap();
        assert_eq!(winner_second, b"b");

        db.switch_active_profile("default");
        rebuild_deployment(&game, &mut db).unwrap();
        let winner_back = fs::read(game.data_path.join("textures/same.dds")).unwrap();
        assert_eq!(winner_back, b"a");
    }

    #[cfg(unix)]
    #[test]
    fn profile_switch_changes_plugins_txt_state() {
        let temp = TempDir::new().unwrap();
        let game = test_game(&temp);
        let mut db = ModDatabase::default();
        add_mod(&game, &mut db, "plugins", "Data/SwitchTest.esp", b"x", true);
        rebuild_deployment(&game, &mut db).unwrap();
        db.plugin_disabled.remove("SwitchTest.esp");
        db.write_plugins_txt(&game).unwrap();
        let default_plugins = fs::read_to_string(game.plugins_txt_path().unwrap()).unwrap();
        assert!(default_plugins.contains("*SwitchTest.esp"));

        db.switch_active_profile("profile_two");
        db.plugin_disabled.insert("SwitchTest.esp".to_string());
        rebuild_deployment(&game, &mut db).unwrap();
        db.write_plugins_txt(&game).unwrap();
        let other_plugins = fs::read_to_string(game.plugins_txt_path().unwrap()).unwrap();
        assert!(other_plugins.lines().any(|l| l.trim() == "SwitchTest.esp"));
    }
}
