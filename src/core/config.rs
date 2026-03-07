use crate::core::games::Game;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
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

fn default_active_profile_id() -> String {
    "default".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub version: u32,
    pub first_run: bool,
    pub current_game_id: Option<String>,
    pub nexus_api_key: Option<String>,
    pub games: Vec<Game>,
    /// User-chosen directory that holds the `downloads/` sub-folder (and
    /// optionally future extracted-mods storage).  When `None`, defaults to
    /// `~/.local/share/linkmm`.
    #[serde(default)]
    pub app_data_dir: Option<PathBuf>,
    /// File names of archives that have already been installed as mods.
    /// Used by the Downloads page to show / hide installed archives.
    #[serde(default)]
    pub installed_archives: Vec<String>,
    #[serde(default = "default_profiles")]
    pub profiles: Vec<Profile>,
    #[serde(default = "default_active_profile_id")]
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
                Ok(contents) => match serde_json::from_str::<AppConfig>(&contents) {
                    Ok(mut config) => {
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

    pub fn save(&self) {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                log::error!("Failed to create config directory: {e}");
                return;
            }
        }
        match serde_json::to_string_pretty(self) {
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

    /// Set `mods_base_dir` on every game based on the user-chosen
    /// `app_data_dir`.  Must be called after loading or after adding games /
    /// changing `app_data_dir`.
    pub fn apply_mods_base_dirs(&mut self) {
        for game in &mut self.games {
            game.mods_base_dir = self.app_data_dir.clone();
        }
    }

    /// Returns the directory where downloaded archives are stored.
    ///
    /// When `app_data_dir` is configured this is `<app_data_dir>/downloads/`;
    /// otherwise it falls back to `~/.local/share/linkmm/downloads/`.
    pub fn downloads_dir(&self) -> PathBuf {
        match &self.app_data_dir {
            Some(dir) => dir.join("downloads"),
            None => dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("linkmm")
                .join("downloads"),
        }
    }

    pub fn config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("linkmm")
            .join("config.json")
    }

    pub fn current_game(&self) -> Option<&Game> {
        let id = self.current_game_id.as_deref()?;
        self.games.iter().find(|g| g.id == id)
    }
}
