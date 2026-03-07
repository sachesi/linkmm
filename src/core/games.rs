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

    /// Canonical vanilla master plugins for this game, in load-order priority.
    pub fn vanilla_masters(&self) -> &'static [&'static str] {
        match self {
            GameKind::SkyrimSE | GameKind::SkyrimLE => &[
                "Skyrim.esm",
                "Update.esm",
                "Dawnguard.esm",
                "HearthFires.esm",
                "Dragonborn.esm",
            ],
            GameKind::Fallout4 => &[
                "Fallout4.esm",
                "DLCRobot.esm",
                "DLCworkshop01.esm",
                "DLCCoast.esm",
                "DLCworkshop02.esm",
                "DLCworkshop03.esm",
                "DLCNukaWorld.esm",
            ],
            GameKind::Fallout3 => &["Fallout3.esm"],
            GameKind::FalloutNV => &[
                "FalloutNV.esm",
                "DeadMoney.esm",
                "HonestHearts.esm",
                "OldWorldBlues.esm",
                "LonesomeRoad.esm",
                "GunRunnersArsenal.esm",
                "CaravanPack.esm",
                "ClassicPack.esm",
                "MercenaryPack.esm",
                "TribalPack.esm",
            ],
            GameKind::Oblivion => &["Oblivion.esm"],
        }
    }

    /// Sub-directory under `%LOCALAPPDATA%` (or its Linux/Proton equivalent)
    /// where the game stores its `plugins.txt`.
    pub fn local_app_data_folder(&self) -> &'static str {
        match self {
            GameKind::SkyrimSE => "Skyrim Special Edition",
            GameKind::SkyrimLE => "Skyrim",
            GameKind::Fallout4 => "Fallout4",
            GameKind::Fallout3 => "Fallout3",
            GameKind::FalloutNV => "FalloutNV",
            GameKind::Oblivion => "Oblivion",
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
    /// When set, mods are stored under `<mods_base_dir>/mods/<game_id>/`
    /// instead of the default `~/.local/share/linkmm/mods/<game_id>/`.
    /// Populated from `AppConfig::app_data_dir` at load time.
    #[serde(skip)]
    pub mods_base_dir: Option<PathBuf>,
}

impl Game {
    pub fn new(kind: GameKind, root_path: PathBuf) -> Self {
        let data_path = root_path.join(kind.default_data_subdir());
        let id = kind.id_str().to_string();
        let name = kind.display_name().to_string();
        Self {
            id,
            name,
            kind,
            root_path,
            data_path,
            mods_base_dir: None,
        }
    }

    /// Return the directory where mod folders for this game are stored.
    ///
    /// When `mods_base_dir` is set (from the user-configured `app_data_dir`),
    /// returns `<mods_base_dir>/mods/<game_id>/`.  Otherwise falls back to
    /// `~/.local/share/linkmm/mods/<game_id>/`.
    pub fn mods_dir(&self) -> PathBuf {
        match &self.mods_base_dir {
            Some(base) => base.join("mods").join(&self.id),
            None => dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("linkmm")
                .join("mods")
                .join(&self.id),
        }
    }

    /// Try to locate the directory that contains `plugins.txt` for this game.
    ///
    /// Checks Steam/Proton compatdata paths common on Linux.  Returns `None`
    /// when no matching directory is found.
    pub fn plugins_txt_dir(&self) -> Option<PathBuf> {
        let app_id = self.kind.steam_app_id()?;
        let home = dirs::home_dir()?;
        let sub = self.kind.local_app_data_folder();

        // Common Proton / Steam-on-Linux compatdata roots
        let roots: &[&str] = &[
            ".steam/steam/steamapps/compatdata",
            ".local/share/Steam/steamapps/compatdata",
            "snap/steam/common/.steam/steam/steamapps/compatdata",
            ".var/app/com.valvesoftware.Steam/.steam/steam/steamapps/compatdata",
        ];

        for root in roots {
            let path = home
                .join(root)
                .join(app_id.to_string())
                .join("pfx/drive_c/users/steamuser/AppData/Local")
                .join(sub);
            if path.is_dir() {
                return Some(path);
            }
        }
        None
    }

    /// Return the expected path of `plugins.txt`, even if it does not yet exist.
    /// Returns `None` only if the AppData directory cannot be determined at all.
    pub fn plugins_txt_path(&self) -> Option<PathBuf> {
        Some(self.plugins_txt_dir()?.join("plugins.txt"))
    }
}
