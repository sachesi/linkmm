use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    pub fn is_phase1_steam_redirector_target(&self) -> bool {
        matches!(self, GameKind::SkyrimSE)
    }

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

    /// Canonical Steam App ID used for launching and runtime integration.
    pub fn primary_steam_app_id(&self) -> Option<u32> {
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

    /// All recognized Steam App IDs for detection/library scanning.
    ///
    /// The first ID is always the canonical launch App ID returned by
    /// [`Self::primary_steam_app_id`].
    pub fn steam_app_ids(&self) -> &'static [u32] {
        match self {
            GameKind::SkyrimSE => &[489830],
            GameKind::SkyrimVR => &[611670],
            GameKind::SkyrimLE => &[72850],
            GameKind::Fallout4 => &[377160],
            GameKind::Fallout4VR => &[611660],
            GameKind::Fallout3 => &[22300],
            // 22490 is Fallout: New Vegas PCR (regional SKU) and maps to the
            // same managed game kind as canonical app 22380.
            GameKind::FalloutNV => &[22380, 22490],
            GameKind::Oblivion => &[22330],
            GameKind::Morrowind => &[22320],
            GameKind::Starfield => &[1716740],
        }
    }

    /// Backwards-compatible alias for the canonical Steam App ID.
    pub fn steam_app_id(&self) -> Option<u32> {
        self.primary_steam_app_id()
    }

    /// Resolve a known Steam App ID to a supported game kind.
    pub fn from_steam_app_id(app_id: u32) -> Option<GameKind> {
        Self::all()
            .into_iter()
            .find(|kind| kind.steam_app_ids().contains(&app_id))
    }

    pub fn default_data_subdir(&self) -> &str {
        "Data"
    }

    /// Whether this game uses the standard `plugins.txt` load-order format.
    ///
    /// Morrowind stores its plugin list in `Morrowind.ini` under `[Game Files]`,
    /// not in a separate `plugins.txt`.  All other supported games use the
    /// standard `*PluginName.ext` (enabled) / `PluginName.ext` (disabled) format.
    pub fn has_plugins_txt(&self) -> bool {
        !matches!(self, GameKind::Morrowind)
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
            self.primary_steam_app_id()
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

    /// Canonical game and common loader executable names for this game kind.
    pub fn expected_executable_names(&self) -> &'static [&'static str] {
        match self {
            GameKind::SkyrimSE => &["SkyrimSE.exe", "SkyrimSELauncher.exe", "skse64_loader.exe"],
            GameKind::SkyrimVR => &["SkyrimVR.exe", "sksevr_loader.exe"],
            GameKind::SkyrimLE => &["TESV.exe", "SkyrimLauncher.exe", "skse_loader.exe"],
            GameKind::Oblivion => &["Oblivion.exe", "OblivionLauncher.exe", "obse_loader.exe"],
            GameKind::Morrowind => &["Morrowind.exe", "Morrowind Launcher.exe", "MWSE.exe"],
            GameKind::Fallout4 => &["Fallout4.exe", "Fallout4Launcher.exe", "f4se_loader.exe"],
            GameKind::Fallout4VR => &["Fallout4VR.exe", "f4sevr_loader.exe"],
            GameKind::FalloutNV => &[
                "FalloutNV.exe",
                "FalloutNVLauncher.exe",
                "nvse_loader.exe",
                "xnvse_loader.exe",
            ],
            GameKind::Fallout3 => &["Fallout3.exe", "FalloutLauncher.exe", "fose_loader.exe"],
            GameKind::Starfield => &["Starfield.exe", "sfse_loader.exe"],
        }
    }

    pub fn phase1_steam_launch_candidates(&self) -> &'static [&'static str] {
        match self {
            GameKind::SkyrimSE => &["SkyrimSELauncher.exe", "SkyrimSE.exe", "skse64_loader.exe"],
            _ => &[],
        }
    }

    pub fn phase1_steam_target_label(&self, exe_name: &str) -> &'static str {
        match (self, exe_name) {
            (GameKind::SkyrimSE, "SkyrimSELauncher.exe") => "Skyrim launcher",
            (GameKind::SkyrimSE, "SkyrimSE.exe") => "SkyrimSE.exe",
            (GameKind::SkyrimSE, "skse64_loader.exe") => "SKSE",
            _ => "Custom target",
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
    /// Actual Steam App ID for this specific Steam game instance.
    ///
    /// This may differ from [`GameKind::primary_steam_app_id`] for alternate
    /// Steam SKUs (for example Fallout NV PCR / 22490).
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steam_app_id: Option<u32>,
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
        let app_id = kind
            .primary_steam_app_id()
            .expect("all managed Steam game kinds have a canonical Steam App ID");
        Self::new_steam_with_app_id(kind, root_path, app_id)
    }

    pub fn new_steam_with_app_id(kind: GameKind, root_path: PathBuf, steam_app_id: u32) -> Self {
        let data_path = root_path.join(kind.default_data_subdir());
        let id = Uuid::new_v4().to_string();
        let name = kind.display_name().to_string();
        Self {
            id,
            name,
            kind,
            launcher_source: GameLauncherSource::Steam,
            steam_app_id: Some(steam_app_id),
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
            steam_app_id: None,
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

    /// Steam App ID for this concrete game instance.
    ///
    /// For Steam instances this prefers the persisted per-instance app ID and
    /// falls back to the canonical game-kind ID for backwards compatibility
    /// with older config files that predate per-instance storage.
    pub fn steam_instance_app_id(&self) -> Option<u32> {
        match self.launcher_source {
            GameLauncherSource::Steam => self
                .steam_app_id
                .or_else(|| self.kind.primary_steam_app_id()),
            GameLauncherSource::NonSteamUmu => None,
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
        if !self.kind.has_plugins_txt() {
            return None;
        }
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

                // UMU (and Wine/GE-Proton) place the prefix contents directly
                // at WINEPREFIX, i.e. prefix/drive_c/... with no pfx/ layer.
                // Some users point UMU at a Steam-style compatdata directory
                // (which has a pfx/ subdirectory), so we check both.
                let path = prefix
                    .join("drive_c")
                    .join("users")
                    .join("steamuser")
                    .join("AppData")
                    .join("Local")
                    .join(sub);
                if path.is_dir() {
                    return Some(path);
                }
                // Fallback: Steam-style prefix (compatdata/pfx/drive_c/...).
                let path_pfx = prefix
                    .join("pfx")
                    .join("drive_c")
                    .join("users")
                    .join("steamuser")
                    .join("AppData")
                    .join("Local")
                    .join(sub);
                if path_pfx.is_dir() {
                    return Some(path_pfx);
                }
                // Prefix not yet initialised by umu-run.  Return the expected
                // path so write_plugins_txt can create the directory tree.
                Some(path)
            }
            GameLauncherSource::Steam => {
                let app_id = self.steam_instance_app_id()?;
                let compatdata = crate::core::steam::find_compatdata_path(app_id)?;
                let path = compatdata
                    .join("pfx")
                    .join("drive_c")
                    .join("users")
                    .join("steamuser")
                    .join("AppData")
                    .join("Local")
                    .join(sub);
                // Return path even if the directory does not exist yet — the
                // Proton prefix may not be fully initialised on a fresh install.
                // write_plugins_txt creates the parent directories as needed.
                Some(path)
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

    #[test]
    fn non_steam_plugins_resolution_prefers_native_umu_layout_over_pfx() {
        // UMU's native layout is prefix/drive_c/... (no pfx/).
        // If both layouts exist, the native one should win.
        let temp = TempDir::new().unwrap();
        let prefix = temp.path().join("prefix");
        let local_appdata_sub = GameKind::SkyrimSE.local_app_data_folder();

        let native_target = prefix
            .join("drive_c")
            .join("users")
            .join("steamuser")
            .join("AppData")
            .join("Local")
            .join(local_appdata_sub);
        std::fs::create_dir_all(&native_target).unwrap();

        // Also create the pfx-style layout so we know the native one wins.
        let pfx_target = prefix
            .join("pfx")
            .join("drive_c")
            .join("users")
            .join("steamuser")
            .join("AppData")
            .join("Local")
            .join(local_appdata_sub);
        std::fs::create_dir_all(&pfx_target).unwrap();

        let game = Game::new_non_steam_umu(
            GameKind::SkyrimSE,
            temp.path().join("root"),
            UmuGameConfig {
                exe_path: temp.path().join("SkyrimSE.exe"),
                prefix_path: Some(prefix.clone()),
                proton_path: None,
            },
        );
        assert_eq!(game.plugins_txt_dir(), Some(native_target));
    }

    #[test]
    fn plugins_txt_dir_returns_expected_path_even_when_not_yet_initialised() {
        // On a fresh install the prefix/AppData dir may not exist yet.
        // plugins_txt_dir should still return the expected path so the
        // caller can create it.
        let temp = TempDir::new().unwrap();
        let prefix = temp.path().join("prefix"); // intentionally not created
        let game = Game::new_non_steam_umu(
            GameKind::SkyrimSE,
            temp.path().join("root"),
            UmuGameConfig {
                exe_path: temp.path().join("SkyrimSE.exe"),
                prefix_path: Some(prefix.clone()),
                proton_path: None,
            },
        );
        let expected = prefix
            .join("drive_c")
            .join("users")
            .join("steamuser")
            .join("AppData")
            .join("Local")
            .join(GameKind::SkyrimSE.local_app_data_folder());
        assert_eq!(game.plugins_txt_dir(), Some(expected));
    }

    #[test]
    fn morrowind_has_no_plugins_txt() {
        assert!(!GameKind::Morrowind.has_plugins_txt());
        let game = Game::new_non_steam_umu(
            GameKind::Morrowind,
            PathBuf::from("/tmp/morrowind"),
            UmuGameConfig {
                exe_path: PathBuf::from("/tmp/morrowind/Morrowind.exe"),
                prefix_path: None,
                proton_path: None,
            },
        );
        assert!(game.plugins_txt_dir().is_none());
        assert!(game.plugins_txt_path().is_none());
    }

    #[test]
    fn expected_executable_names_include_script_extender_loaders() {
        let skyrim = GameKind::SkyrimSE.expected_executable_names();
        assert!(skyrim.contains(&"SkyrimSE.exe"));
        assert!(skyrim.contains(&"skse64_loader.exe"));

        let fallout4 = GameKind::Fallout4.expected_executable_names();
        assert!(fallout4.contains(&"Fallout4.exe"));
        assert!(fallout4.contains(&"f4se_loader.exe"));
    }

    #[test]
    fn fallout_nv_alias_app_id_maps_to_same_game_kind() {
        assert_eq!(
            GameKind::from_steam_app_id(22380),
            Some(GameKind::FalloutNV)
        );
        assert_eq!(
            GameKind::from_steam_app_id(22490),
            Some(GameKind::FalloutNV)
        );
    }

    #[test]
    fn primary_steam_app_id_remains_canonical_for_fallout_nv() {
        assert_eq!(GameKind::FalloutNV.primary_steam_app_id(), Some(22380));
        assert_eq!(GameKind::FalloutNV.steam_app_ids(), &[22380, 22490]);
    }

    #[test]
    fn steam_instance_app_id_tracks_detected_instance_id() {
        let pcr =
            Game::new_steam_with_app_id(GameKind::FalloutNV, PathBuf::from("/tmp/pcr"), 22490);
        assert_eq!(pcr.steam_instance_app_id(), Some(22490));
        assert_eq!(pcr.kind.primary_steam_app_id(), Some(22380));
    }

    #[test]
    fn legacy_steam_instance_without_saved_app_id_falls_back_to_primary() {
        let mut game = Game::new_steam(GameKind::FalloutNV, PathBuf::from("/tmp/fnv"));
        game.steam_app_id = None;
        assert_eq!(game.steam_instance_app_id(), Some(22380));
    }

    #[test]
    fn steam_app_id_serialization_is_instance_specific() {
        let steam =
            Game::new_steam_with_app_id(GameKind::FalloutNV, PathBuf::from("/tmp/pcr"), 22490);
        let steam_json = serde_json::to_value(&steam).expect("serialize steam game");
        assert_eq!(
            steam_json.get("steam_app_id"),
            Some(&serde_json::json!(22490))
        );

        let non_steam = Game::new_non_steam_umu(
            GameKind::SkyrimSE,
            PathBuf::from("/tmp/nonsteam"),
            UmuGameConfig {
                exe_path: PathBuf::from("/tmp/nonsteam/SkyrimSE.exe"),
                prefix_path: None,
                proton_path: None,
            },
        );
        let non_steam_json = serde_json::to_value(&non_steam).expect("serialize non-steam game");
        assert!(non_steam_json.get("steam_app_id").is_none());
    }

    #[test]
    fn fallout_nv_and_pcr_instances_share_family_metadata() {
        let base =
            Game::new_steam_with_app_id(GameKind::FalloutNV, PathBuf::from("/tmp/fnv"), 22380);
        let pcr =
            Game::new_steam_with_app_id(GameKind::FalloutNV, PathBuf::from("/tmp/fnv_pcr"), 22490);

        assert_eq!(base.steam_instance_app_id(), Some(22380));
        assert_eq!(pcr.steam_instance_app_id(), Some(22490));
        assert_eq!(base.kind.nexus_game_id(), "newvegas");
        assert_eq!(pcr.kind.nexus_game_id(), "newvegas");
        assert_eq!(base.kind.vanilla_masters(), pcr.kind.vanilla_masters());
        assert_eq!(
            base.kind.expected_executable_names(),
            pcr.kind.expected_executable_names()
        );
    }
}
