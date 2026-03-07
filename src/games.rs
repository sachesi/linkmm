use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GameKind {
    SkyrimSE,
    SkyrimLE,
    Fallout4,
    Fallout3,
    FalloutNV,
    Oblivion,
}

impl GameKind {
    pub fn display_name(&self) -> &str {
        match self {
            GameKind::SkyrimSE => "The Elder Scrolls V: Skyrim Special Edition",
            GameKind::SkyrimLE => "The Elder Scrolls V: Skyrim",
            GameKind::Fallout4 => "Fallout 4",
            GameKind::Fallout3 => "Fallout 3",
            GameKind::FalloutNV => "Fallout: New Vegas",
            GameKind::Oblivion => "The Elder Scrolls IV: Oblivion",
        }
    }

    pub fn steam_app_id(&self) -> Option<u32> {
        match self {
            GameKind::SkyrimSE => Some(489830),
            GameKind::SkyrimLE => Some(72850),
            GameKind::Fallout4 => Some(377160),
            GameKind::Fallout3 => Some(22300),
            GameKind::FalloutNV => Some(22380),
            GameKind::Oblivion => Some(22330),
        }
    }

    pub fn default_data_subdir(&self) -> &str {
        "Data"
    }

    pub fn all() -> Vec<GameKind> {
        vec![
            GameKind::SkyrimSE,
            GameKind::SkyrimLE,
            GameKind::Fallout4,
            GameKind::Fallout3,
            GameKind::FalloutNV,
            GameKind::Oblivion,
        ]
    }

    pub fn id_str(&self) -> &str {
        match self {
            GameKind::SkyrimSE => "skyrim_se",
            GameKind::SkyrimLE => "skyrim_le",
            GameKind::Fallout4 => "fallout4",
            GameKind::Fallout3 => "fallout3",
            GameKind::FalloutNV => "fallout_nv",
            GameKind::Oblivion => "oblivion",
        }
    }

    pub fn nexus_game_id(&self) -> &str {
        match self {
            GameKind::SkyrimSE => "skyrimspecialedition",
            GameKind::SkyrimLE => "skyrim",
            GameKind::Fallout4 => "fallout4",
            GameKind::Fallout3 => "fallout3",
            GameKind::FalloutNV => "newvegas",
            GameKind::Oblivion => "oblivion",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Game {
    pub id: String,
    pub name: String,
    pub kind: GameKind,
    pub root_path: PathBuf,
    pub data_path: PathBuf,
}

impl Game {
    pub fn new(kind: GameKind, root_path: PathBuf) -> Self {
        let data_path = root_path.join(kind.default_data_subdir());
        let id = format!("{}_{}", kind.id_str(), root_path.to_string_lossy().len());
        let name = kind.display_name().to_string();
        Self {
            id,
            name,
            kind,
            root_path,
            data_path,
        }
    }

    pub fn mods_dir(&self) -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("linkmm")
            .join("mods")
            .join(&self.id)
    }
}
