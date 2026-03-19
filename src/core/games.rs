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

    /// Ordered list of well-known executable names for this game.
    ///
    /// The first entry is the primary (vanilla) game executable.  Later
    /// entries are the launcher and any script-extender loaders (SKSE, F4SE,
    /// NVSE, FOSE, OBSE) that live in the game root directory.
    pub fn known_executables(&self) -> &'static [&'static str] {
        match self {
            GameKind::SkyrimSE => &[
                "SkyrimSE.exe",
                "SkyrimSELauncher.exe",
                "skse64_loader.exe",
            ],
            GameKind::SkyrimLE => &[
                "TESV.exe",
                "SkyrimLauncher.exe",
                "skse_loader.exe",
            ],
            GameKind::Fallout4 => &[
                "Fallout4.exe",
                "Fallout4Launcher.exe",
                "f4se_loader.exe",
            ],
            GameKind::Fallout3 => &[
                "Fallout3.exe",
                "Fallout3Launcher.exe",
                "fose_loader.exe",
            ],
            GameKind::FalloutNV => &[
                "FalloutNV.exe",
                "FalloutNVLauncher.exe",
                "nvse_loader.exe",
            ],
            GameKind::Oblivion => &[
                "Oblivion.exe",
                "OblivionLauncher.exe",
                "obse_loader.exe",
            ],
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
    /// Always computed from `root_path` at load time; never persisted to JSON
    /// so it stays in sync even when the user changes the game root path.
    #[serde(skip)]
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
    /// Uses [`crate::core::steam::find_compatdata_path`] to locate the correct
    /// Proton prefix for this game — that is, the `compatdata/<app_id>` entry
    /// in the Steam library that actually holds the game — and then navigates
    /// to the equivalent of `%LOCALAPPDATA%/<game_folder>` inside it.
    ///
    /// Returns `None` when no matching directory is found.
    pub fn plugins_txt_dir(&self) -> Option<PathBuf> {
        let app_id = self.kind.steam_app_id()?;
        let sub = self.kind.local_app_data_folder();

        let compatdata = crate::core::steam::find_compatdata_path(app_id)?;
        let path = compatdata
            .join("pfx")
            .join("drive_c")
            .join("users")
            .join("steamuser")
            .join("AppData")
            .join("Local")
            .join(sub);
        if path.is_dir() {
            Some(path)
        } else {
            None
        }
    }

    /// Return the expected path of `plugins.txt`, even if it does not yet exist.
    /// Returns `None` only if the AppData directory cannot be determined at all.
    pub fn plugins_txt_path(&self) -> Option<PathBuf> {
        Some(self.plugins_txt_dir()?.join("plugins.txt"))
    }

    /// Returns the subset of [`GameKind::known_executables`] that actually
    /// exist as files inside [`Game::root_path`], in their canonical order.
    ///
    /// The first item (if any) is always the primary game executable; later
    /// items are launchers and script-extender loaders installed by the user.
    pub fn discover_executables(&self) -> Vec<String> {
        self.kind
            .known_executables()
            .iter()
            .filter(|name| self.root_path.join(name).is_file())
            .map(|&name| name.to_string())
            .collect()
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_executables_are_non_empty_for_all_games() {
        for kind in GameKind::all() {
            let exes = kind.known_executables();
            assert!(!exes.is_empty(), "{:?} has no known executables", kind);
            // Primary exe must end with .exe
            assert!(
                exes[0].ends_with(".exe"),
                "primary exe for {:?} should end with .exe",
                kind
            );
        }
    }

    #[test]
    fn discover_executables_returns_only_existing_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Create two of the three known SkyrimSE executables
        std::fs::write(tmp.path().join("SkyrimSE.exe"), b"").unwrap();
        std::fs::write(tmp.path().join("skse64_loader.exe"), b"").unwrap();

        let game = Game::new(GameKind::SkyrimSE, tmp.path().to_path_buf());
        let exes = game.discover_executables();

        assert_eq!(exes, vec!["SkyrimSE.exe", "skse64_loader.exe"]);
    }

    #[test]
    fn discover_executables_returns_empty_when_none_exist() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let game = Game::new(GameKind::Fallout4, tmp.path().to_path_buf());
        assert!(game.discover_executables().is_empty());
    }
}
