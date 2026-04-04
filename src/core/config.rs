use crate::core::games::Game;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
}

/// Configuration for a single external tool (e.g., BodySlide, xEdit).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolOutputMode {
    DedicatedDirectory,
    SnapshotGameData,
}

fn default_tool_output_mode() -> ToolOutputMode {
    ToolOutputMode::SnapshotGameData
}

fn default_run_profile() -> String {
    "default".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfig {
    /// Unique identifier for this tool.
    pub id: String,
    /// User-friendly name for this tool.
    pub name: String,
    /// Path to the Windows .exe file.
    pub exe_path: PathBuf,
    /// Optional command-line arguments.
    #[serde(default)]
    pub arguments: String,
    /// Steam App ID to determine which game's Proton prefix to use.
    pub app_id: u32,
    /// Generated output capture strategy for this tool.
    #[serde(default = "default_tool_output_mode")]
    pub output_mode: ToolOutputMode,
    /// Preferred output directory for tools that support explicit output paths.
    #[serde(default)]
    pub managed_output_dir: Option<PathBuf>,
    /// Tool run profile identifier used when replacing generated output packages.
    #[serde(default = "default_run_profile")]
    pub run_profile: String,
}

impl Profile {
    /// Create a new profile with a unique ID derived from the name and current time.
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let slug: String = name
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '_' })
            .collect();
        let id = format!("{}_{}", slug, timestamp);
        Self { id, name }
    }

    pub fn default_profile() -> Self {
        Self {
            id: "default".to_string(),
            name: "Default".to_string(),
        }
    }
}

fn default_profiles() -> Vec<Profile> {
    vec![Profile::default_profile()]
}

fn default_true() -> bool {
    true
}

pub fn default_active_profile_id() -> String {
    "default".to_string()
}

/// Returns the default profiles list; used as a fallback when no game is active.
pub fn default_active_profile_id_vec() -> Vec<Profile> {
    default_profiles()
}

// ── Per-game settings ─────────────────────────────────────────────────────────

/// Settings that are specific to a single managed game.
///
/// Stored in `AppConfig::game_settings` keyed by the game's `id` string.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameSettings {
    /// User-chosen directory that holds the `downloads/` sub-folder and
    /// extracted-mod storage for this game.  When `None`, defaults to
    /// `~/.local/share/linkmm`.
    #[serde(default)]
    pub app_data_dir: Option<PathBuf>,
    /// File names of archives that have already been installed as mods for
    /// this game.  Used by the Downloads page to show / hide installed
    /// archives.
    #[serde(default)]
    pub installed_archives: Vec<String>,
    /// Mod profiles defined for this game.
    #[serde(default = "default_profiles")]
    pub profiles: Vec<Profile>,
    /// The profile currently active for this game.
    #[serde(default = "default_active_profile_id")]
    pub active_profile_id: String,
    /// External tools configured for this game.
    #[serde(default)]
    pub tools: Vec<ToolConfig>,
}

impl Default for GameSettings {
    fn default() -> Self {
        Self {
            app_data_dir: None,
            installed_archives: Vec::new(),
            profiles: default_profiles(),
            active_profile_id: default_active_profile_id(),
            tools: Vec::new(),
        }
    }
}

// ── AppConfig ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub version: u32,
    pub first_run: bool,
    pub current_game_id: Option<String>,
    pub nexus_api_key: Option<String>,
    pub games: Vec<Game>,
    /// Per-game settings keyed by game ID.
    #[serde(default)]
    pub game_settings: HashMap<String, GameSettings>,

    // ── Logging preferences ───────────────────────────────────────────────
    /// The release tag of the currently installed `umu-run` binary, e.g. `"1.4.0"`.
    ///
    /// Persisted in `config.toml` so the app can detect on next launch whether
    /// a newer release is available and re-download automatically.
    /// `None` means umu-run has not been downloaded yet.
    #[serde(default)]
    pub umu_installed_version: Option<String>,

    /// Show mod activity (Info-level) log entries in the log viewer.
    #[serde(default = "default_true")]
    pub log_activity: bool,
    /// Show warning-level log entries in the log viewer.
    #[serde(default = "default_true")]
    pub log_warnings: bool,
    /// Show error-level log entries in the log viewer.
    #[serde(default = "default_true")]
    pub log_errors: bool,
    /// Show info-level log entries in the log viewer.
    #[serde(default = "default_true")]
    pub log_info: bool,
    /// Show debug-level log entries in the log viewer.
    #[serde(default = "default_true")]
    pub log_debug: bool,

    // ── Legacy global fields – kept for migration only, never re-serialized ──
    /// Migrated into `game_settings[*].app_data_dir`.
    #[serde(default, skip_serializing)]
    pub app_data_dir: Option<PathBuf>,
    /// Migrated into `game_settings[*].installed_archives`.
    #[serde(default, skip_serializing)]
    pub installed_archives: Vec<String>,
    /// Migrated into `game_settings[*].profiles`.
    #[serde(default = "default_profiles", skip_serializing)]
    pub profiles: Vec<Profile>,
    /// Migrated into `game_settings[*].active_profile_id`.
    #[serde(default = "default_active_profile_id", skip_serializing)]
    pub active_profile_id: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: 1,
            first_run: true,
            current_game_id: None,
            nexus_api_key: None,
            games: Vec::new(),
            game_settings: HashMap::new(),
            log_activity: true,
            log_warnings: true,
            log_errors: true,
            log_info: true,
            log_debug: true,
            umu_installed_version: None,
            // Legacy fields – only meaningful during migration
            app_data_dir: None,
            installed_archives: Vec::new(),
            profiles: default_profiles(),
            active_profile_id: default_active_profile_id(),
        }
    }
}

impl AppConfig {
    pub fn load_or_default() -> Self {
        let path = Self::config_path();
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(contents) => match toml::from_str::<AppConfig>(&contents) {
                    Ok(mut config) => {
                        config.migrate_legacy_global_settings();
                        config.apply_mods_base_dirs();
                        return config;
                    }
                    Err(e) => {
                        log::warn!("Failed to parse config: {e}, using defaults");
                    }
                },
                Err(e) => {
                    log::warn!("Failed to read config file: {e}, using defaults");
                }
            }
        }
        Self::default()
    }

    /// Migrate from the pre-per-game config format.
    ///
    /// If `game_settings` is empty but the old global fields (`app_data_dir`,
    /// `installed_archives`, `profiles`, `active_profile_id`) are present,
    /// copy them into the per-game settings for every configured game so no
    /// data is lost on upgrade.
    fn migrate_legacy_global_settings(&mut self) {
        if !self.game_settings.is_empty() || self.games.is_empty() {
            return;
        }
        let legacy = GameSettings {
            app_data_dir: self.app_data_dir.clone(),
            installed_archives: self.installed_archives.clone(),
            profiles: if self.profiles.is_empty() {
                default_profiles()
            } else {
                self.profiles.clone()
            },
            active_profile_id: if self.active_profile_id.is_empty() {
                default_active_profile_id()
            } else {
                self.active_profile_id.clone()
            },
            tools: Vec::new(),
        };
        for game in &self.games {
            self.game_settings
                .entry(game.id.clone())
                .or_insert_with(|| legacy.clone());
        }
    }

    pub fn save(&self) {
        let path = Self::config_path();
        if let Some(parent) = path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            log::error!("Failed to create config directory: {e}");
            return;
        }
        match toml::to_string_pretty(self) {
            Ok(contents) => {
                if let Err(e) = std::fs::write(&path, contents) {
                    log::error!("Failed to write config file: {e}");
                }
            }
            Err(e) => {
                log::error!("Failed to serialize config: {e}");
            }
        }
    }

    /// Access per-game settings for `game_id`, returning `None` if no entry
    /// exists yet.
    pub fn game_settings_ref(&self, game_id: &str) -> Option<&GameSettings> {
        self.game_settings.get(game_id)
    }

    /// Mutable access to per-game settings for `game_id`, creating a default
    /// entry if one does not yet exist.
    pub fn game_settings_mut(&mut self, game_id: &str) -> &mut GameSettings {
        self.game_settings.entry(game_id.to_string()).or_default()
    }

    /// Set `mods_base_dir` on every game based on the per-game `app_data_dir`.
    /// Must be called after loading or after adding games / changing settings.
    pub fn apply_mods_base_dirs(&mut self) {
        let gs_snapshot: HashMap<String, Option<PathBuf>> = self
            .game_settings
            .iter()
            .map(|(id, gs)| (id.clone(), gs.app_data_dir.clone()))
            .collect();
        for game in &mut self.games {
            game.mods_base_dir = gs_snapshot.get(&game.id).and_then(|d| d.clone());
            game.data_path = game.root_path.join(game.kind.default_data_subdir());
        }
    }

    /// Returns the directory where downloaded archives are stored for a
    /// specific game.
    ///
    /// Uses the per-game `app_data_dir` when configured; otherwise falls back
    /// to `~/.local/share/linkmm/downloads/<game_id>/`.
    ///
    /// When `managed_game` is `None` or empty (after trimming whitespace),
    /// returns the base downloads directory without a game subfolder.
    pub fn downloads_dir(&self, managed_game: Option<&str>) -> PathBuf {
        let game_id = managed_game.map(str::trim).filter(|id| !id.is_empty());
        let base = match game_id.and_then(|id| self.game_settings.get(id)) {
            Some(gs) => match &gs.app_data_dir {
                Some(dir) => dir.join("downloads"),
                None => dirs::data_local_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("linkmm")
                    .join("downloads"),
            },
            None => dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("linkmm")
                .join("downloads"),
        };
        match game_id {
            Some(id) => base.join(id),
            None => base,
        }
    }

    pub fn config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("linkmm")
            .join("config.toml")
    }

    pub fn current_game(&self) -> Option<&Game> {
        let id = self.current_game_id.as_deref()?;
        self.games.iter().find(|g| g.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downloads_dir_appends_managed_game_subdirectory() {
        let mut cfg = AppConfig::default();
        cfg.game_settings.insert(
            "skyrim_se".to_string(),
            GameSettings {
                app_data_dir: Some(PathBuf::from("/tmp/linkmm")),
                ..GameSettings::default()
            },
        );
        assert_eq!(
            cfg.downloads_dir(Some("skyrim_se")),
            PathBuf::from("/tmp/linkmm/downloads/skyrim_se")
        );
    }

    #[test]
    fn downloads_dir_without_game_returns_base_downloads_path() {
        let mut cfg = AppConfig::default();
        cfg.game_settings.insert(
            "skyrim_se".to_string(),
            GameSettings {
                app_data_dir: Some(PathBuf::from("/tmp/linkmm")),
                ..GameSettings::default()
            },
        );
        // No game specified → falls back to the default location (no app_data_dir applies)
        let base = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("linkmm")
            .join("downloads");
        assert_eq!(cfg.downloads_dir(None), base);
        assert_eq!(cfg.downloads_dir(Some("   ")), base);
    }

    #[test]
    fn migrate_legacy_global_settings_populates_per_game() {
        let mut cfg = AppConfig {
            games: vec![crate::core::games::Game::new(
                crate::core::games::GameKind::SkyrimSE,
                PathBuf::from("/games/skyrim"),
            )],
            app_data_dir: Some(PathBuf::from("/data/linkmm")),
            installed_archives: vec!["mod.zip".to_string()],
            profiles: vec![Profile::default_profile()],
            active_profile_id: "default".to_string(),
            ..AppConfig::default()
        };
        cfg.migrate_legacy_global_settings();
        let gs = cfg.game_settings.get("skyrim_se").unwrap();
        assert_eq!(gs.app_data_dir, Some(PathBuf::from("/data/linkmm")));
        assert_eq!(gs.installed_archives, vec!["mod.zip"]);
    }

    #[test]
    fn migrate_legacy_does_not_overwrite_existing_game_settings() {
        let mut cfg = AppConfig {
            games: vec![crate::core::games::Game::new(
                crate::core::games::GameKind::SkyrimSE,
                PathBuf::from("/games/skyrim"),
            )],
            app_data_dir: Some(PathBuf::from("/old/data")),
            ..AppConfig::default()
        };
        // Pre-populate game_settings so migration should be skipped
        cfg.game_settings.insert(
            "skyrim_se".to_string(),
            GameSettings {
                app_data_dir: Some(PathBuf::from("/new/data")),
                ..GameSettings::default()
            },
        );
        cfg.migrate_legacy_global_settings();
        // Should NOT have been overwritten
        assert_eq!(
            cfg.game_settings.get("skyrim_se").unwrap().app_data_dir,
            Some(PathBuf::from("/new/data"))
        );
    }
}
