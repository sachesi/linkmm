use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use crate::games::Game;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub version: u32,
    pub first_run: bool,
    pub current_game_id: Option<String>,
    pub nexus_api_key: Option<String>,
    pub games: Vec<Game>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: 1,
            first_run: true,
            current_game_id: None,
            nexus_api_key: None,
            games: Vec::new(),
        }
    }
}

impl AppConfig {
    pub fn load_or_default() -> Self {
        let path = Self::config_path();
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(contents) => {
                    match serde_json::from_str::<AppConfig>(&contents) {
                        Ok(config) => return config,
                        Err(e) => {
                            log::warn!("Failed to parse config: {e}, using defaults");
                        }
                    }
                }
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
