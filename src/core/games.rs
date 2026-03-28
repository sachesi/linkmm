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

/// Entry in the per-game NXM metadata file, recording the Nexus mod ID and
/// game domain for a downloaded archive.
#[derive(Debug, Serialize, Deserialize)]
struct NxmEntry {
    game_domain: String,
    mod_id: u32,
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

    // ── Per-game configuration directory ─────────────────────────────────────

    /// Return the directory used to store per-game configuration files such as
    /// `mods.json` and `nxm_metadata.json`.
    ///
    /// Always resolves to `~/.config/linkmm/<game_id>/`, independent of the
    /// user-configured `app_data_dir` / `mods_base_dir`.
    pub fn config_dir(&self) -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("linkmm")
            .join(&self.id)
    }

    // ── NXM download metadata ─────────────────────────────────────────────────

    /// Read the Nexus mod ID associated with `archive_name` from the
    /// consolidated per-game metadata file
    /// `~/.config/linkmm/<game_id>/nxm_metadata.toml`.
    ///
    /// Returns `None` if the file does not exist, the archive is not listed,
    /// or the stored game domain does not match this game.
    pub fn read_nxm_mod_id(&self, archive_name: &str) -> Option<u32> {
        let path = self.config_dir().join("nxm_metadata.toml");
        let contents = std::fs::read_to_string(&path).ok()?;
        let map: std::collections::HashMap<String, NxmEntry> =
            toml::from_str(&contents).ok()?;
        let entry = map.get(archive_name)?;
        if entry.game_domain != self.kind.nexus_game_id() {
            return None;
        }
        Some(entry.mod_id)
    }

    /// Write or update the Nexus mod ID for `archive_name` in the consolidated
    /// per-game metadata file
    /// `~/.config/linkmm/<game_id>/nxm_metadata.toml`.
    ///
    /// Creates the file and its parent directory if they do not exist.
    pub fn write_nxm_mod_id(
        &self,
        archive_name: &str,
        game_domain: &str,
        mod_id: u32,
    ) -> Result<(), String> {
        let dir = self.config_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create game config dir: {e}"))?;
        let path = dir.join("nxm_metadata.toml");
        let mut map: std::collections::HashMap<String, NxmEntry> = if path.exists() {
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|c| toml::from_str(&c).ok())
                .unwrap_or_default()
        } else {
            std::collections::HashMap::new()
        };
        map.insert(
            archive_name.to_string(),
            NxmEntry {
                game_domain: game_domain.to_string(),
                mod_id,
            },
        );
        let body = toml::to_string_pretty(&map)
            .map_err(|e| format!("Failed to serialize NXM metadata: {e}"))?;
        std::fs::write(&path, body)
            .map_err(|e| format!("Failed to write {}: {e}", path.display()))
    }

    /// Remove the NXM metadata entry for `archive_name` from the consolidated
    /// per-game metadata file, if it exists.
    pub fn remove_nxm_mod_id(&self, archive_name: &str) {
        let path = self.config_dir().join("nxm_metadata.toml");
        if !path.exists() {
            return;
        }
        let Ok(contents) = std::fs::read_to_string(&path) else {
            return;
        };
        let Ok(mut map) =
            toml::from_str::<std::collections::HashMap<String, NxmEntry>>(&contents)
        else {
            return;
        };
        if map.remove(archive_name).is_none() {
            return; // nothing to do
        }
        if let Ok(body) = toml::to_string_pretty(&map) {
            let _ = std::fs::write(&path, body);
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
}
