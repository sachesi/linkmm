use crate::core::games::Game;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ── Plugin types ─────────────────────────────────────────────────────────────

/// Kind of a Bethesda plugin file, determined by file extension.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PluginKind {
    /// `.esm` – Elder Scrolls Master / Fallout Master.  Loads before regular plugins.
    Master,
    /// `.esl` – Light master.  Shares the master load-order slot but has a 512-record limit.
    Light,
    /// `.esp` – Regular plugin.
    Plugin,
}

impl PluginKind {
    pub fn label(&self) -> &'static str {
        match self {
            PluginKind::Master => "ESM",
            PluginKind::Light => "ESL",
            PluginKind::Plugin => "ESP",
        }
    }

    /// Lower value = loads earlier.  Used to sort non-vanilla plugins by type.
    pub fn sort_priority(&self) -> u8 {
        match self {
            PluginKind::Master => 0,
            PluginKind::Light => 1,
            PluginKind::Plugin => 2,
        }
    }
}

/// A single plugin file found in the game's Data directory.
#[derive(Debug, Clone)]
pub struct PluginFile {
    pub name: String,
    pub kind: PluginKind,
    /// Whether this plugin is active in `plugins.txt` (defaults to enabled if not tracked).
    pub enabled: bool,
    /// True for hardcoded vanilla game masters (e.g. `Skyrim.esm`).
    pub is_vanilla: bool,
}

// ── Mod struct ────────────────────────────────────────────────────────────────

/// Generate a UUID-like unique identifier for mod folder names.
///
/// Uses nanosecond timestamp, process ID, and an atomic counter to ensure
/// uniqueness even when multiple mods are created in rapid succession.
fn generate_mod_uuid() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = ts.as_secs();
    let nanos = ts.subsec_nanos();
    let pid = std::process::id();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);

    // Format as 8-4-4-4-12 hex UUID-like string
    // a: seconds (lower 32 bits)
    // b: subsec nanos (upper 16 bits)
    // c: subsec nanos (lower 16 bits)
    // d: pid XOR counter
    // e: seconds (upper 32 bits) + pid + counter
    let a = secs as u32;
    let b = (nanos >> 16) as u16;
    let c = nanos as u16;
    let d = (pid as u16) ^ (seq as u16);
    let e = (((secs >> 32) as u64) << 32) | ((pid as u64) << 16) | (seq as u64);
    format!("{a:08x}-{b:04x}-{c:04x}-{d:04x}-{e:012x}")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mod {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub enabled: bool,
    pub priority: i32,
    pub nexus_id: Option<u32>,
    pub source_path: PathBuf,
    /// True when this mod was downloaded through the Downloads page via the Nexus API.
    #[serde(default)]
    pub installed_from_nexus: bool,
    /// True when the mod should be linked into the game root directory instead
    /// of the Data directory (e.g. mods containing a top-level `Data/` folder
    /// or executables that sit next to the game binary).
    #[serde(default)]
    pub install_to_root: bool,
}

impl Mod {
    pub fn new(name: impl Into<String>, source_path: PathBuf) -> Self {
        let name = name.into();
        let id = generate_mod_uuid();
        Self {
            id,
            name,
            version: None,
            enabled: false,
            priority: 0,
            nexus_id: None,
            source_path,
            installed_from_nexus: false,
            install_to_root: false,
        }
    }
}

// ── ModDatabase ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModDatabase {
    pub mods: Vec<Mod>,
    /// Ordered mod IDs (legacy – kept for compatibility).
    pub load_order: Vec<String>,
    /// Ordered plugin file names for the Load Order page.
    #[serde(default)]
    pub plugin_load_order: Vec<String>,
    /// Plugin file names that the user has explicitly *disabled* in the Load Order.
    #[serde(default)]
    pub plugin_disabled: Vec<String>,
}

impl ModDatabase {
    pub fn load(game: &Game) -> Self {
        let path = game.mods_dir().join("mods.json");
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(contents) => match serde_json::from_str::<ModDatabase>(&contents) {
                    Ok(db) => return db,
                    Err(e) => {
                        log::warn!("Failed to parse mods database: {e}, using empty database");
                    }
                },
                Err(e) => {
                    log::warn!("Failed to read mods database: {e}");
                }
            }
        }
        Self::default()
    }

    pub fn save(&self, game: &Game) {
        let mods_dir = game.mods_dir();
        if let Err(e) = std::fs::create_dir_all(&mods_dir) {
            log::error!("Failed to create mods directory: {e}");
            return;
        }
        let path = mods_dir.join("mods.json");
        match serde_json::to_string_pretty(self) {
            Ok(contents) => {
                if let Err(e) = std::fs::write(&path, contents) {
                    log::error!("Failed to write mods database: {e}");
                }
            }
            Err(e) => {
                log::error!("Failed to serialize mods database: {e}");
            }
        }
    }

    pub fn scan_mods_dir(&mut self, game: &Game) {
        let mods_dir = game.mods_dir();
        if !mods_dir.is_dir() {
            return;
        }
        // Remove stale entries whose source directories no longer exist
        self.mods.retain(|m| m.source_path.is_dir());
    }

    // ── Plugin / Load-Order helpers ──────────────────────────────────────────

    /// Scan the game's `Data` directory for `.esm`, `.esl` and `.esp` files.
    ///
    /// `enabled` is derived from `plugin_disabled`: a plugin is enabled unless
    /// it appears in that list.  Vanilla masters are always marked `is_vanilla = true`.
    pub fn scan_plugins(&self, game: &Game) -> Vec<PluginFile> {
        let mut plugins = Vec::new();
        let data_dir = &game.data_path;
        if !data_dir.is_dir() {
            return plugins;
        }
        let vanilla = game.kind.vanilla_masters();
        if let Ok(entries) = std::fs::read_dir(data_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let lower = name.to_lowercase();
                let kind = if lower.ends_with(".esm") {
                    PluginKind::Master
                } else if lower.ends_with(".esl") {
                    PluginKind::Light
                } else if lower.ends_with(".esp") {
                    PluginKind::Plugin
                } else {
                    continue;
                };
                let is_vanilla = vanilla.contains(&name.as_str());
                let enabled = !self.plugin_disabled.contains(&name);
                plugins.push(PluginFile {
                    name,
                    kind,
                    enabled,
                    is_vanilla,
                });
            }
        }
        plugins
    }

    /// Return plugins in load-order sequence.
    ///
    /// Vanilla masters come first (in their canonical game order), then the
    /// remaining plugins follow `plugin_load_order`; any plugins not yet
    /// tracked are appended at the end.
    pub fn get_ordered_plugins(&self, game: &Game) -> Vec<PluginFile> {
        let plugins = self.scan_plugins(game);

        // Partition into vanilla masters and the rest
        let vanilla_order = game.kind.vanilla_masters();
        let (mut vanilla, mut rest): (Vec<_>, Vec<_>) =
            plugins.into_iter().partition(|p| p.is_vanilla);

        // Sort vanilla masters in their canonical order
        vanilla.sort_by_key(|p| {
            vanilla_order
                .iter()
                .position(|&v| v == p.name.as_str())
                .unwrap_or(usize::MAX)
        });

        // Apply saved order to non-vanilla plugins
        let mut ordered: Vec<PluginFile> = Vec::new();
        for name in &self.plugin_load_order {
            if let Some(idx) = rest.iter().position(|p| &p.name == name) {
                ordered.push(rest.remove(idx));
            }
        }
        // Any plugin not yet in plugin_load_order: sort by type priority then name
        rest.sort_by(|a, b| {
            a.kind
                .sort_priority()
                .cmp(&b.kind.sort_priority())
                .then_with(|| a.name.cmp(&b.name))
        });
        ordered.extend(rest);

        let mut result = vanilla;
        result.extend(ordered);
        result
    }

    /// Sort non-vanilla plugins by type priority (ESM → ESL → ESP) then
    /// alphabetically (case-insensitive) within each type.
    ///
    /// Vanilla masters are always kept first in their canonical game order and
    /// are never reordered.  After sorting, `plugin_load_order` is updated and
    /// the database can be saved / written to `plugins.txt` by the caller.
    pub fn sort_plugins_by_type(&mut self, game: &Game) {
        let mut plugins = self.get_ordered_plugins(game);
        // Vanilla plugins are placed first by get_ordered_plugins; find where
        // the non-vanilla section starts.
        let vanilla_end = plugins.iter().take_while(|p| p.is_vanilla).count();
        plugins[vanilla_end..].sort_by_cached_key(|p| {
            (p.kind.sort_priority(), p.name.to_lowercase())
        });
        self.set_plugin_order(&plugins);
    }

    /// Update `plugin_load_order` and `plugin_disabled` from the given ordered list.
    pub fn set_plugin_order(&mut self, plugins: &[PluginFile]) {
        self.plugin_load_order = plugins
            .iter()
            .filter(|p| !p.is_vanilla)
            .map(|p| p.name.clone())
            .collect();
        self.plugin_disabled = plugins
            .iter()
            .filter(|p| !p.enabled)
            .map(|p| p.name.clone())
            .collect();
    }

    /// Write `plugins.txt` to the game's AppData directory (Proton/Windows path).
    ///
    /// Format follows limo/Bethesda convention: `*Plugin.esm` (enabled) or
    /// `Plugin.esp` (disabled).  A comment header is included for clarity.
    pub fn write_plugins_txt(&self, game: &Game) -> Result<(), String> {
        let Some(plugins_path) = game.plugins_txt_path() else {
            return Ok(()); // Path unknown – skip silently
        };
        if let Some(parent) = plugins_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create plugins directory: {e}"))?;
        }
        let plugins = self.get_ordered_plugins(game);
        let mut content =
            String::from("# This file is used by the game to determine which plugins are active.\n");
        for plugin in &plugins {
            if plugin.enabled {
                content.push('*');
            }
            content.push_str(&plugin.name);
            content.push('\n');
        }
        std::fs::write(&plugins_path, content)
            .map_err(|e| format!("Failed to write plugins.txt: {e}"))
    }

    /// Read `plugins.txt` (if present) and synchronise `plugin_load_order` and
    /// `plugin_disabled` with the order and activation state it declares.
    pub fn sync_from_plugins_txt(&mut self, game: &Game) {
        let Some(plugins_path) = game.plugins_txt_path() else {
            return;
        };
        if !plugins_path.exists() {
            return;
        }
        let Ok(contents) = std::fs::read_to_string(&plugins_path) else {
            return;
        };
        let mut order: Vec<String> = Vec::new();
        let mut disabled: Vec<String> = Vec::new();
        for line in contents.lines() {
            // `str::lines()` already strips `\n` and `\r\n`; trim remaining
            // whitespace so we handle any exotic endings gracefully.
            let line = line.trim();
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            if let Some(name) = line.strip_prefix('*') {
                order.push(name.to_string());
            } else {
                order.push(line.to_string());
                disabled.push(line.to_string());
            }
        }
        if !order.is_empty() {
            self.plugin_load_order = order;
            self.plugin_disabled = disabled;
        }
    }
}

pub struct ModManager;

impl ModManager {
    pub fn enable_mod(game: &Game, mod_entry: &Mod) -> Result<(), String> {
        let target_dir = if mod_entry.install_to_root {
            &game.root_path
        } else {
            &game.data_path
        };
        if !target_dir.is_dir() {
            std::fs::create_dir_all(target_dir)
                .map_err(|e| format!("Failed to create target directory: {e}"))?;
        }
        link_directory_contents(&mod_entry.source_path, target_dir)
    }

    pub fn disable_mod(game: &Game, mod_entry: &Mod) -> Result<(), String> {
        let target_dir = if mod_entry.install_to_root {
            &game.root_path
        } else {
            &game.data_path
        };
        unlink_directory_contents(&mod_entry.source_path, target_dir)
    }

    pub fn create_mod_directory(game: &Game) -> Result<PathBuf, String> {
        let uuid = generate_mod_uuid();
        let dir = game.mods_dir().join(&uuid);
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create mod directory: {e}"))?;
        Ok(dir)
    }
}

fn link_directory_contents(source: &Path, dest: &Path) -> Result<(), String> {
    if !source.is_dir() {
        return Err(format!("Source is not a directory: {}", source.display()));
    }
    std::fs::create_dir_all(dest)
        .map_err(|e| format!("Failed to create destination directory: {e}"))?;

    let entries =
        std::fs::read_dir(source).map_err(|e| format!("Failed to read source directory: {e}"))?;

    for entry in entries.flatten() {
        let src_path = entry.path();
        let file_name = entry.file_name();
        let dest_path = dest.join(&file_name);

        if src_path.is_dir() {
            link_directory_contents(&src_path, &dest_path)?;
        } else {
            #[cfg(unix)]
            {
                use std::os::unix::fs::symlink;
                if dest_path.is_symlink() {
                    // Remove a broken symlink at the destination so we can re-create it
                    if !dest_path.exists() {
                        std::fs::remove_file(&dest_path).map_err(|e| {
                            format!("Failed to remove broken symlink {:?}: {e}", dest_path)
                        })?;
                    } else {
                        // Valid symlink or file already exists — skip
                        continue;
                    }
                } else if dest_path.exists() {
                    // A real file exists — skip to avoid overwriting it
                    continue;
                }
                symlink(&src_path, &dest_path).map_err(|e| {
                    format!(
                        "Failed to create symlink {:?} -> {:?}: {e}",
                        dest_path, src_path
                    )
                })?;
            }
            #[cfg(not(unix))]
            {
                return Err("Symlinks are only supported on Unix systems".to_string());
            }
        }
    }
    Ok(())
}

fn unlink_directory_contents(source: &Path, dest: &Path) -> Result<(), String> {
    if !source.is_dir() {
        return Ok(());
    }
    if !dest.is_dir() {
        return Ok(());
    }

    let entries =
        std::fs::read_dir(source).map_err(|e| format!("Failed to read source directory: {e}"))?;

    for entry in entries.flatten() {
        let src_path = entry.path();
        let file_name = entry.file_name();
        let dest_path = dest.join(&file_name);

        if src_path.is_dir() {
            unlink_directory_contents(&src_path, &dest_path)?;
        } else {
            #[cfg(unix)]
            {
                // Only remove the symlink if it points to our source file
                if dest_path.is_symlink() {
                    if let Ok(target) = std::fs::read_link(&dest_path) {
                        if target == src_path {
                            std::fs::remove_file(&dest_path).map_err(|e| {
                                format!("Failed to remove symlink {:?}: {e}", dest_path)
                            })?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_generation_is_unique() {
        let mut ids = std::collections::HashSet::new();
        for _ in 0..100 {
            let id = generate_mod_uuid();
            assert!(!ids.contains(&id), "duplicate UUID: {id}");
            ids.insert(id);
        }
    }

    #[test]
    fn uuid_format_looks_like_uuid() {
        let id = generate_mod_uuid();
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(parts.len(), 5, "UUID should have 5 dash-separated parts: {id}");
        assert_eq!(parts[0].len(), 8);
        assert_eq!(parts[1].len(), 4);
        assert_eq!(parts[2].len(), 4);
        assert_eq!(parts[3].len(), 4);
        assert_eq!(parts[4].len(), 12);
        assert!(
            id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
            "UUID should be hex: {id}"
        );
    }

    #[test]
    fn mod_new_generates_uuid_id() {
        let m1 = Mod::new("Test Mod", PathBuf::from("/tmp/test1"));
        let m2 = Mod::new("Test Mod", PathBuf::from("/tmp/test2"));
        // Same name but different UUIDs
        assert_ne!(m1.id, m2.id);
        assert_eq!(m1.name, "Test Mod");
        assert_eq!(m2.name, "Test Mod");
    }
}
