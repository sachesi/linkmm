use crate::core::deployment;
use crate::core::games::Game;
use libloot::{Game as LootGame, GameType as LootGameType};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
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
    let e = ((secs >> 32) << 32) | ((pid as u64) << 16) | (seq as u64);
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
    /// The name of the archive file this mod was installed from.
    #[serde(default)]
    pub archive_name: Option<String>,
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
            archive_name: None,
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
    pub plugin_disabled: HashSet<String>,
}

impl ModDatabase {
    fn loot_game_type(game: &Game) -> LootGameType {
        match game.kind {
            crate::core::games::GameKind::SkyrimSE => LootGameType::SkyrimSE,
            crate::core::games::GameKind::SkyrimLE => LootGameType::Skyrim,
            crate::core::games::GameKind::Fallout4 => LootGameType::Fallout4,
            crate::core::games::GameKind::Fallout3 => LootGameType::Fallout3,
            crate::core::games::GameKind::FalloutNV => LootGameType::FalloutNV,
            crate::core::games::GameKind::Oblivion => LootGameType::Oblivion,
        }
    }

    fn try_sort_with_loot(plugins: &[PluginFile], game: &Game) -> Result<Vec<String>, String> {
        let local_path = game
            .plugins_txt_dir()
            .unwrap_or_else(|| game.root_path.clone());
        let loot_game_type = Self::loot_game_type(game);
        let mut loot_game =
            LootGame::with_local_path(loot_game_type, &game.root_path, &local_path).map_err(
                |e| {
                    format!(
                        "Failed to create libloot game handle (type: {loot_game_type:?}, root: {}, local: {}): {e}",
                        game.root_path.display(),
                        local_path.display()
                    )
                },
            )?;

        let plugin_paths: Vec<PathBuf> = plugins
            .iter()
            .map(|plugin| game.data_path.join(&plugin.name))
            .collect();
        let plugin_path_refs: Vec<&Path> = plugin_paths.iter().map(PathBuf::as_path).collect();
        loot_game.load_plugins(&plugin_path_refs).map_err(|e| {
            format!(
                "Failed to load plugins for libloot sorting from {}: {e}",
                game.data_path.display()
            )
        })?;

        let plugin_names: Vec<&str> = plugins.iter().map(|plugin| plugin.name.as_str()).collect();
        loot_game
            .sort_plugins(&plugin_names)
            .map_err(|e| format!("Failed to sort plugins with libloot for {}: {e}", game.id))
    }

    fn sort_plugins_fallback_by_type(plugins: &mut [PluginFile]) {
        plugins.sort_by_cached_key(|p| (p.kind.sort_priority(), p.name.to_lowercase()));
    }

    /// Path to the `mods.toml` configuration file for this game.
    ///
    /// Always located at `~/.config/linkmm/<game_id>/mods.toml`.
    fn db_path(game: &Game) -> std::path::PathBuf {
        game.config_dir().join("mods.toml")
    }

    pub fn load(game: &Game) -> Self {
        let path = Self::db_path(game);
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(contents) => match toml::from_str::<ModDatabase>(&contents) {
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
        let dir = game.config_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            log::error!("Failed to create game config directory: {e}");
            return;
        }
        let path = Self::db_path(game);
        match toml::to_string_pretty(self) {
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
        let vanilla: HashSet<&str> = game.kind.vanilla_masters().iter().copied().collect();
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
                let is_vanilla = vanilla.contains(name.as_str());
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
        let (mut vanilla, rest): (Vec<_>, Vec<_>) = plugins.into_iter().partition(|p| p.is_vanilla);

        // Sort vanilla masters in their canonical order
        vanilla.sort_by_key(|p| {
            vanilla_order
                .iter()
                .position(|&v| v == p.name.as_str())
                .unwrap_or(usize::MAX)
        });

        // Apply saved order to non-vanilla plugins
        let load_order_indices: HashMap<&str, usize> = self
            .plugin_load_order
            .iter()
            .enumerate()
            .map(|(idx, name)| (name.as_str(), idx))
            .collect();
        let mut ordered_with_idx: Vec<(usize, PluginFile)> = Vec::new();
        let mut unordered: Vec<PluginFile> = Vec::new();
        for plugin in rest {
            if let Some(idx) = load_order_indices.get(plugin.name.as_str()) {
                ordered_with_idx.push((*idx, plugin));
            } else {
                unordered.push(plugin);
            }
        }
        ordered_with_idx.sort_by_key(|(idx, _)| *idx);
        let mut ordered: Vec<PluginFile> = ordered_with_idx
            .into_iter()
            .map(|(_, plugin)| plugin)
            .collect();
        // Any plugin not yet in plugin_load_order: sort by type priority then name
        unordered.sort_by(|a, b| {
            a.kind
                .sort_priority()
                .cmp(&b.kind.sort_priority())
                .then_with(|| a.name.cmp(&b.name))
        });
        ordered.extend(unordered);

        let mut result = vanilla;
        result.extend(ordered);
        result
    }

    /// Sort non-vanilla plugins using libloot and fall back to deterministic
    /// type sorting (ESM → ESL → ESP, then case-insensitive name) if libloot
    /// cannot sort the current plugin set.
    ///
    /// Vanilla masters are always kept first in their canonical game order and
    /// are never reordered.  After sorting, `plugin_load_order` is updated and
    /// the database can be saved / written to `plugins.txt` by the caller.
    pub fn sort_plugins_by_type(&mut self, game: &Game) {
        let plugins = self.get_ordered_plugins(game);
        // Vanilla plugins are placed first by get_ordered_plugins; find where
        // the non-vanilla section starts.
        let vanilla_end = plugins.iter().take_while(|p| p.is_vanilla).count();
        let mut non_vanilla = plugins[vanilla_end..].to_vec();

        match Self::try_sort_with_loot(&non_vanilla, game) {
            Ok(sorted_names) => {
                let mut by_name: HashMap<String, PluginFile> = non_vanilla
                    .into_iter()
                    .map(|plugin| (plugin.name.to_lowercase(), plugin))
                    .collect();
                let mut sorted_non_vanilla = Vec::with_capacity(sorted_names.len());
                for name in sorted_names {
                    if let Some(plugin) = by_name.remove(&name.to_lowercase()) {
                        sorted_non_vanilla.push(plugin);
                    }
                }
                let mut remaining: Vec<PluginFile> = by_name.into_values().collect();
                Self::sort_plugins_fallback_by_type(&mut remaining);
                sorted_non_vanilla.extend(remaining);
                let mut ordered = plugins[..vanilla_end].to_vec();
                ordered.extend(sorted_non_vanilla);
                self.set_plugin_order(&ordered);
            }
            Err(e) => {
                log::warn!("{e}; falling back to type-based plugin sorting");
                Self::sort_plugins_fallback_by_type(&mut non_vanilla);
                let mut ordered = plugins[..vanilla_end].to_vec();
                ordered.extend(non_vanilla);
                self.set_plugin_order(&ordered);
            }
        }
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
        let mut content = String::from(
            "# This file is used by the game to determine which plugins are active.\n",
        );
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
            // `plugins.txt` should not contain duplicates; collect into a set so
            // malformed files cannot create redundant disabled entries.
            self.plugin_disabled = disabled.into_iter().collect();
        }
    }
}

pub struct ModManager;

impl ModManager {
    pub fn enable_mod(game: &Game, mod_entry: &Mod) -> Result<(), String> {
        // Use new deployment module with improved link management
        let report = deployment::deploy_mod(game, mod_entry)?;
        log::info!(
            "Deployed mod '{}': {} data links, {} root links",
            mod_entry.name,
            report.data_links_created,
            report.root_links_created
        );
        Ok(())
    }

    pub fn disable_mod(game: &Game, mod_entry: &Mod) -> Result<(), String> {
        Self::disable_mod_internal(game, mod_entry, true)
    }

    /// Disable a mod without running the legacy nested Data/Data cleanup.
    ///
    /// Use this in batch undeploy paths, then call `purge_legacy_nested_data_dir`
    /// once after all mods are processed to avoid repeated full-directory scans.
    pub fn disable_mod_without_legacy_cleanup(game: &Game, mod_entry: &Mod) -> Result<(), String> {
        Self::disable_mod_internal(game, mod_entry, false)
    }

    /// Clean up legacy symlinks from game Data/Data left by older deployment logic.
    ///
    /// Batch deploy/undeploy flows should call this once after unlinking mods.
    pub fn purge_legacy_nested_data_dir(game: &Game) {
        deployment::cleanup_legacy_nested_data(game);
    }

    fn disable_mod_internal(
        game: &Game,
        mod_entry: &Mod,
        run_legacy_cleanup: bool,
    ) -> Result<(), String> {
        // Use new deployment module with improved link management
        let report = deployment::undeploy_mod(game, mod_entry)?;
        log::info!(
            "Undeployed mod '{}': removed {} data links, {} root links",
            mod_entry.name,
            report.data_links_removed,
            report.root_links_removed
        );

        // Legacy cleanup if requested
        if run_legacy_cleanup {
            deployment::cleanup_legacy_nested_data(game);
        }

        Ok(())
    }

    pub fn create_mod_directory(game: &Game) -> Result<PathBuf, String> {
        let uuid = generate_mod_uuid();
        let dir = game.mods_dir().join(&uuid);
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create mod directory: {e}"))?;
        Ok(dir)
    }

    /// Fully uninstall a mod: remove its symlinks from the game directory,
    /// delete its files from disk, and remove its entry from the database.
    pub fn uninstall_mod(game: &Game, mod_entry: &Mod) -> Result<(), String> {
        // Undeploy first so no dangling symlinks remain in the game directory.
        // Log but do not abort on undeploy failure – we still want to clean up
        // the files and database record.
        if let Err(e) = Self::disable_mod(game, mod_entry) {
            log::warn!(
                "Undeploy warning during uninstall of '{}': {e}",
                mod_entry.name
            );
        }

        // Delete the mod's managed directory from disk.
        if mod_entry.source_path.exists() {
            std::fs::remove_dir_all(&mod_entry.source_path)
                .map_err(|e| format!("Failed to delete mod files for '{}': {e}", mod_entry.name))?;
        }

        // Remove the mod entry from the database.
        let mut db = ModDatabase::load(game);
        db.mods.retain(|m| m.id != mod_entry.id);
        db.save(game);

        Ok(())
    }
}


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
        assert_eq!(
            parts.len(),
            5,
            "UUID should have 5 dash-separated parts: {id}"
        );
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

    #[cfg(unix)]
    fn tempdir() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static CTR: AtomicU32 = AtomicU32::new(0);
        let n = CTR.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("linkmm_mods_test_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[cfg(unix)]
    #[test]
    fn get_ordered_plugins_respects_saved_order_and_sorts_untracked() {
        use crate::core::games::{Game, GameKind};

        let tmp = tempdir();
        let game_root = tmp.join("game");
        let data_dir = game_root.join("Data");
        let mods_base = tmp.join("mods_base");
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::create_dir_all(mods_base.join("mods").join("test_game")).unwrap();
        for file in ["a.esm", "b.esl", "c.esp", "z.esp"] {
            std::fs::write(data_dir.join(file), b"plugin").unwrap();
        }

        let game = Game {
            id: "test_game".to_string(),
            name: "Test Game".to_string(),
            kind: GameKind::SkyrimSE,
            root_path: game_root.clone(),
            data_path: data_dir,
            mods_base_dir: Some(mods_base),
        };
        let db = ModDatabase {
            plugin_load_order: vec!["z.esp".to_string(), "a.esm".to_string()],
            plugin_disabled: ["z.esp".to_string()].into_iter().collect(),
            ..ModDatabase::default()
        };

        let ordered = db.get_ordered_plugins(&game);
        let names: Vec<&str> = ordered.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["z.esp", "a.esm", "b.esl", "c.esp"]);
        assert!(!ordered[0].enabled);
        assert!(ordered[1].enabled);
    }

    #[test]
    fn plugin_disabled_deserialize_deduplicates_entries() {
        let json = r#"{
            "mods": [],
            "load_order": [],
            "plugin_load_order": [],
            "plugin_disabled": ["A.esp", "A.esp", "B.esm"]
        }"#;

        let db: ModDatabase = serde_json::from_str(json).unwrap();
        assert_eq!(db.plugin_disabled.len(), 2);
        assert!(db.plugin_disabled.contains("A.esp"));
        assert!(db.plugin_disabled.contains("B.esm"));

        let encoded = serde_json::to_string(&db).unwrap();
        assert_eq!(encoded.matches("A.esp").count(), 1);
        assert_eq!(encoded.matches("B.esm").count(), 1);
    }

    #[test]
    fn sort_plugins_by_type_falls_back_when_libloot_cannot_parse_plugins() {
        use crate::core::games::{Game, GameKind};

        let tmp = tempdir();
        let game_root = tmp.join("game");
        let data_dir = game_root.join("Data");
        let mods_base = tmp.join("mods_base");
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::create_dir_all(mods_base.join("mods").join("test_game")).unwrap();

        for file in ["z.esp", "a.esm", "b.esl", "c.esp"] {
            std::fs::write(data_dir.join(file), b"not-a-real-plugin").unwrap();
        }

        let game = Game {
            id: "test_game".to_string(),
            name: "Test Game".to_string(),
            kind: GameKind::SkyrimSE,
            root_path: game_root,
            data_path: data_dir,
            mods_base_dir: Some(mods_base),
        };

        let mut db = ModDatabase {
            plugin_load_order: vec!["z.esp".to_string(), "a.esm".to_string()],
            ..ModDatabase::default()
        };
        db.sort_plugins_by_type(&game);

        assert_eq!(
            db.plugin_load_order,
            vec![
                "a.esm".to_string(),
                "b.esl".to_string(),
                "c.esp".to_string(),
                "z.esp".to_string()
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn uninstall_mod_removes_source_directory_and_database_entry() {
        use crate::core::games::{Game, GameKind};

        let tmp = tempdir();

        // Build a minimal Game pointing at a temp directory.
        let game_root = tmp.join("game");
        let mods_base = tmp.join("mods_base");
        std::fs::create_dir_all(game_root.join("Data")).unwrap();
        // mods_dir() returns mods_base/mods/test_game/
        std::fs::create_dir_all(mods_base.join("mods").join("test_game")).unwrap();

        let game = Game {
            id: "test_game".to_string(),
            name: "Test Game".to_string(),
            kind: GameKind::SkyrimSE,
            root_path: game_root.clone(),
            data_path: game_root.join("Data"),
            mods_base_dir: Some(mods_base.clone()),
        };

        // Create a mod directory with a Data/ subfolder and a file.
        let mod_dir = game.mods_dir().join("test-mod-uuid");
        std::fs::create_dir_all(mod_dir.join("Data").join("textures")).unwrap();
        std::fs::write(
            mod_dir.join("Data").join("textures").join("sky.dds"),
            b"dds",
        )
        .unwrap();

        // Register the mod in the database.
        let mod_entry = crate::core::mods::Mod::new("TestMod", mod_dir.clone());
        let mut db = ModDatabase::load(&game);
        db.mods.push(mod_entry.clone());
        db.save(&game);

        // Deploy the mod manually so we can verify symlinks are cleaned up.
        deployment::deploy_mod(&game, &mod_entry).unwrap();
        assert!(
            game.data_path.join("textures").join("sky.dds").is_symlink()
                || game.data_path.join("textures").join("sky.dds").exists(),
            "link should exist before uninstall"
        );

        // Uninstall.
        ModManager::uninstall_mod(&game, &mod_entry).unwrap();

        // Symlinks cleaned up.
        assert!(
            !game.data_path.join("textures").join("sky.dds").exists(),
            "symlink should be gone after uninstall"
        );
        // Mod directory deleted.
        assert!(!mod_dir.exists(), "mod directory should be deleted");
        // Database entry removed.
        let db_after = ModDatabase::load(&game);
        assert!(
            db_after.mods.iter().all(|m| m.id != mod_entry.id),
            "mod should be removed from database"
        );
    }
}
