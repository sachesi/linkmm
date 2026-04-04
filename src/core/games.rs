use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GameKind {
    SkyrimSE,
    SkyrimVR,
    SkyrimLE,
    Fallout4,
    Fallout4VR,
    Fallout3,
    FalloutNV,
    Oblivion,
    Morrowind,
    Starfield,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameLauncherSource {
    Steam,
    NonSteamUmu,
}

fn default_launcher_source() -> GameLauncherSource {
    GameLauncherSource::Steam
}

impl GameKind {
    pub fn display_name(&self) -> &str {
        match self {
            GameKind::SkyrimSE => "The Elder Scrolls V: Skyrim Special Edition",
            GameKind::SkyrimVR => "The Elder Scrolls V: Skyrim VR",
            GameKind::SkyrimLE => "The Elder Scrolls V: Skyrim",
            GameKind::Fallout4 => "Fallout 4",
            GameKind::Fallout4VR => "Fallout 4 VR",
            GameKind::Fallout3 => "Fallout 3",
            GameKind::FalloutNV => "Fallout: New Vegas",
            GameKind::Oblivion => "The Elder Scrolls IV: Oblivion",
            GameKind::Morrowind => "The Elder Scrolls III: Morrowind",
            GameKind::Starfield => "Starfield",
        }
    }

    pub fn steam_app_id(&self) -> Option<u32> {
        match self {
            GameKind::SkyrimSE => Some(489830),
            GameKind::SkyrimVR => Some(611670),
            GameKind::SkyrimLE => Some(72850),
            GameKind::Fallout4 => Some(377160),
            GameKind::Fallout4VR => Some(611660),
            GameKind::Fallout3 => Some(22300),
            GameKind::FalloutNV => Some(22380),
            GameKind::Oblivion => Some(22330),
            GameKind::Morrowind => Some(22320),
            GameKind::Starfield => Some(1716740),
        }
    }

    pub fn default_data_subdir(&self) -> &str {
        "Data"
    }

    pub fn all() -> Vec<GameKind> {
        vec![
            GameKind::SkyrimSE,
            GameKind::SkyrimVR,
            GameKind::SkyrimLE,
            GameKind::Fallout4,
            GameKind::Fallout4VR,
            GameKind::Fallout3,
            GameKind::FalloutNV,
            GameKind::Oblivion,
            GameKind::Morrowind,
            GameKind::Starfield,
        ]
    }

    pub fn id_str(&self) -> &str {
        match self {
            GameKind::SkyrimSE => "skyrim_se",
            GameKind::SkyrimVR => "skyrim_vr",
            GameKind::SkyrimLE => "skyrim_le",
            GameKind::Fallout4 => "fallout4",
            GameKind::Fallout4VR => "fallout4_vr",
            GameKind::Fallout3 => "fallout3",
            GameKind::FalloutNV => "fallout_nv",
            GameKind::Oblivion => "oblivion",
            GameKind::Morrowind => "morrowind",
            GameKind::Starfield => "starfield",
        }
    }

    pub fn nexus_game_id(&self) -> &str {
        match self {
            GameKind::SkyrimSE => "skyrimspecialedition",
            GameKind::SkyrimVR => "skyrimspecialedition",
            GameKind::SkyrimLE => "skyrim",
            GameKind::Fallout4 => "fallout4",
            GameKind::Fallout4VR => "fallout4",
            GameKind::Fallout3 => "fallout3",
            GameKind::FalloutNV => "newvegas",
            GameKind::Oblivion => "oblivion",
            GameKind::Morrowind => "morrowind",
            GameKind::Starfield => "starfield",
        }
    }

    /// Canonical vanilla master plugins for this game, in load-order priority.
    pub fn vanilla_masters(&self) -> &'static [&'static str] {
        match self {
            GameKind::SkyrimSE | GameKind::SkyrimVR | GameKind::SkyrimLE => &[
                "Skyrim.esm",
                "Update.esm",
                "Dawnguard.esm",
                "HearthFires.esm",
                "Dragonborn.esm",
            ],
            GameKind::Fallout4 | GameKind::Fallout4VR => &[
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
            GameKind::Morrowind => &["Morrowind.esm", "Tribunal.esm", "Bloodmoon.esm"],
            GameKind::Starfield => &["Starfield.esm"],
        }
    }

    /// Sub-directory under `%LOCALAPPDATA%` (or its Linux/Proton equivalent)
    /// where the game stores its `plugins.txt`.
    pub fn local_app_data_folder(&self) -> &'static str {
        match self {
            GameKind::SkyrimSE => "Skyrim Special Edition",
            GameKind::SkyrimVR => "Skyrim VR",
            GameKind::SkyrimLE => "Skyrim",
            GameKind::Fallout4 => "Fallout4",
            GameKind::Fallout4VR => "Fallout4VR",
            GameKind::Fallout3 => "Fallout3",
            GameKind::FalloutNV => "FalloutNV",
            GameKind::Oblivion => "Oblivion",
            GameKind::Morrowind => "Morrowind",
            GameKind::Starfield => "Starfield",
        }
    }

    /// Return the `GAMEID` string used by umu-launcher for this game.
    ///
    /// The format is `"umu-<steam_app_id>"`, which enables automatic protonfixes
    /// for the game.  All supported games have a Steam App ID so this always
    /// returns a valid string.
    pub fn umu_game_id(&self) -> String {
        format!(
            "umu-{}",
            self.steam_app_id()
                .expect("all GameKind variants have a Steam App ID")
        )
    }

    /// Try to identify a GameKind from a known game executable filename.
    /// Returns `None` if the executable is not recognized.
    pub fn from_executable(exe_name: &str) -> Option<GameKind> {
        match exe_name {
            // Skyrim SE
            "SkyrimSE.exe" | "SkyrimSELauncher.exe" | "skse64_loader.exe" => {
                Some(GameKind::SkyrimSE)
            }
            // Skyrim VR
            "SkyrimVR.exe" | "sksevr_loader.exe" => Some(GameKind::SkyrimVR),
            // Skyrim LE
            "TESV.exe" | "SkyrimLauncher.exe" | "skse_loader.exe" => Some(GameKind::SkyrimLE),
            // Oblivion
            "Oblivion.exe" | "OblivionLauncher.exe" | "obse_loader.exe" => Some(GameKind::Oblivion),
            // Morrowind
            "Morrowind.exe" | "Morrowind Launcher.exe" | "MWSE.exe" => Some(GameKind::Morrowind),
            // Fallout 4
            "Fallout4.exe" | "Fallout4Launcher.exe" | "f4se_loader.exe" => Some(GameKind::Fallout4),
            // Fallout 4 VR
            "Fallout4VR.exe" | "f4sevr_loader.exe" => Some(GameKind::Fallout4VR),
            // Fallout NV
            "FalloutNV.exe" | "FalloutNVLauncher.exe" | "nvse_loader.exe" | "xnvse_loader.exe" => {
                Some(GameKind::FalloutNV)
            }
            // Fallout 3
            "Fallout3.exe" | "FalloutLauncher.exe" | "fose_loader.exe" => Some(GameKind::Fallout3),
            // Starfield
            "Starfield.exe" | "sfse_loader.exe" => Some(GameKind::Starfield),
            _ => None,
        }
    }
}

/// Configuration for launching a game through umu-launcher (non-Steam).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UmuGameConfig {
    /// Absolute path to the Windows game executable (.exe).
    pub exe_path: PathBuf,
    /// Optional Wine/Proton prefix directory (`WINEPREFIX`).
    ///
    /// When `None`, umu uses its built-in default prefix located at
    /// `~/.local/share/umu/default`, which is created automatically on first
    /// run.  No manual setup is required.
    pub prefix_path: Option<PathBuf>,
    /// Optional explicit path to a Proton installation (`PROTONPATH`).
    ///
    /// When `None`, umu receives `PROTONPATH=GE-Proton` (a magic string that
    /// instructs umu to automatically download the latest GE-Proton release
    /// into `~/.local/share/Steam/compatibilitytools.d/` on first run).
    /// Steam itself does **not** need to be installed for this to work.
    pub proton_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Game {
    pub id: String,
    pub name: String,
    pub kind: GameKind,
    #[serde(default = "default_launcher_source")]
    pub launcher_source: GameLauncherSource,
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
    /// UMU launcher configuration for non-Steam games.
    pub umu_config: Option<UmuGameConfig>,
}

/// Entry in the per-game NXM metadata file, recording the Nexus mod ID and
/// game domain for a downloaded archive.
#[derive(Debug, Serialize, Deserialize)]
struct NxmEntry {
    game_domain: String,
    mod_id: u32,
}

impl Game {
    pub fn new_steam(kind: GameKind, root_path: PathBuf) -> Self {
        let data_path = root_path.join(kind.default_data_subdir());
        let id = Uuid::new_v4().to_string();
        let name = kind.display_name().to_string();
        Self {
            id,
            name,
            kind,
            launcher_source: GameLauncherSource::Steam,
            root_path,
            data_path,
            mods_base_dir: None,
            umu_config: None,
        }
    }

    /// Create a new UMU-based (non-Steam) game from an executable path.
    pub fn new_non_steam_umu(kind: GameKind, root_path: PathBuf, umu_cfg: UmuGameConfig) -> Self {
        let data_path = root_path.join(kind.default_data_subdir());
        let id = Uuid::new_v4().to_string();
        let name = kind.display_name().to_string();
        Self {
            id,
            name,
            kind,
            launcher_source: GameLauncherSource::NonSteamUmu,
            root_path,
            data_path,
            mods_base_dir: None,
            umu_config: Some(umu_cfg),
        }
    }

    pub fn instance_label(&self) -> String {
        match self.launcher_source {
            GameLauncherSource::Steam => format!("{} (Steam)", self.name),
            GameLauncherSource::NonSteamUmu => format!("{} (Non-Steam / UMU)", self.name),
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
        let sub = self.kind.local_app_data_folder();

        match self.launcher_source {
            GameLauncherSource::NonSteamUmu => {
                let umu = self
                    .umu_config
                    .as_ref()
                    .expect("non-steam game must have umu config");
                let prefix = umu
                    .prefix_path
                    .clone()
                    .unwrap_or_else(crate::core::umu::default_wineprefix);
                let path = prefix
                    .join("pfx")
                    .join("drive_c")
                    .join("users")
                    .join("steamuser")
                    .join("AppData")
                    .join("Local")
                    .join(sub);
                if path.is_dir() {
                    return Some(path);
                }
                // Also try without the "pfx" subdirectory in case prefix points directly there
                let path_alt = prefix
                    .join("drive_c")
                    .join("users")
                    .join("steamuser")
                    .join("AppData")
                    .join("Local")
                    .join(sub);
                if path_alt.is_dir() {
                    return Some(path_alt);
                }
                None
            }
            GameLauncherSource::Steam => {
                let app_id = self.kind.steam_app_id()?;
                let compatdata = crate::core::steam::find_compatdata_path(app_id)?;
                let path = compatdata
                    .join("pfx")
                    .join("drive_c")
                    .join("users")
                    .join("steamuser")
                    .join("AppData")
                    .join("Local")
                    .join(sub);
                if path.is_dir() { Some(path) } else { None }
            }
        }
    }

    pub fn validate_umu_setup(&self) -> Result<&UmuGameConfig, String> {
        if self.launcher_source != GameLauncherSource::NonSteamUmu {
            return Err("UMU setup validation is only valid for non-Steam UMU games".to_string());
        }
        let umu_cfg = self
            .umu_config
            .as_ref()
            .ok_or_else(|| "Missing UMU configuration for non-Steam game".to_string())?;
        if !umu_cfg.exe_path.is_file() {
            return Err(format!(
                "UMU game executable does not exist: {}",
                umu_cfg.exe_path.display()
            ));
        }
        if let Some(prefix) = &umu_cfg.prefix_path
            && !prefix.is_dir()
        {
            return Err(format!(
                "UMU Wine prefix path does not exist: {}",
                prefix.display()
            ));
        }
        if let Some(proton) = &umu_cfg.proton_path
            && !proton.is_dir()
        {
            return Err(format!(
                "UMU Proton path does not exist: {}",
                proton.display()
            ));
        }
        if !crate::core::umu::is_umu_available() {
            return Err("umu-run is not installed. Install/update it in Preferences first.".into());
        }
        Ok(umu_cfg)
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
        let map: std::collections::HashMap<String, NxmEntry> = toml::from_str(&contents).ok()?;
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
        std::fs::write(&path, body).map_err(|e| format!("Failed to write {}: {e}", path.display()))
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
        let Ok(mut map) = toml::from_str::<std::collections::HashMap<String, NxmEntry>>(&contents)
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
    use tempfile::TempDir;

    #[test]
    fn steam_and_non_steam_instances_of_same_kind_get_distinct_ids() {
        let steam = Game::new_steam(GameKind::SkyrimSE, PathBuf::from("/tmp/steam_skyrim"));
        let nonsteam = Game::new_non_steam_umu(
            GameKind::SkyrimSE,
            PathBuf::from("/tmp/nonsteam_skyrim"),
            UmuGameConfig {
                exe_path: PathBuf::from("/tmp/nonsteam_skyrim/SkyrimSE.exe"),
                prefix_path: None,
                proton_path: None,
            },
        );
        assert_ne!(steam.id, nonsteam.id);
        assert_eq!(steam.launcher_source, GameLauncherSource::Steam);
        assert_eq!(nonsteam.launcher_source, GameLauncherSource::NonSteamUmu);
    }

    #[test]
    fn non_steam_plugins_resolution_uses_configured_prefix() {
        let temp = TempDir::new().unwrap();
        let prefix = temp.path().join("prefix");
        let target = prefix
            .join("pfx")
            .join("drive_c")
            .join("users")
            .join("steamuser")
            .join("AppData")
            .join("Local")
            .join(GameKind::SkyrimSE.local_app_data_folder());
        std::fs::create_dir_all(&target).unwrap();
        let game = Game::new_non_steam_umu(
            GameKind::SkyrimSE,
            temp.path().join("root"),
            UmuGameConfig {
                exe_path: temp.path().join("SkyrimSE.exe"),
                prefix_path: Some(prefix.clone()),
                proton_path: None,
            },
        );
        assert_eq!(game.plugins_txt_dir(), Some(target));
    }
}
