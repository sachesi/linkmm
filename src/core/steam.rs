use crate::core::games::GameKind;
use std::path::PathBuf;

pub struct SteamLibrary {
    pub path: PathBuf,
    pub apps: Vec<u32>,
}

pub fn find_steam_libraries() -> Vec<SteamLibrary> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return Vec::new(),
    };

    let candidate_roots = vec![
        home.join(".steam").join("steam"),
        home.join(".local").join("share").join("Steam"),
        home.join("snap")
            .join("steam")
            .join("common")
            .join(".steam")
            .join("steam"),
        home.join(".var")
            .join("app")
            .join("com.valvesoftware.Steam")
            .join(".steam")
            .join("steam"),
    ];

    let mut libraries = Vec::new();

    for root in &candidate_roots {
        let steamapps = root.join("steamapps");
        if !steamapps.is_dir() {
            continue;
        }

        // Parse libraryfolders.vdf for additional library paths
        let vdf_path = steamapps.join("libraryfolders.vdf");
        let mut lib_paths = vec![steamapps.clone()];

        if let Ok(contents) = std::fs::read_to_string(&vdf_path) {
            for extra_path in parse_library_folders_vdf(&contents) {
                let extra_steamapps = extra_path.join("steamapps");
                if extra_steamapps.is_dir() && !lib_paths.contains(&extra_steamapps) {
                    lib_paths.push(extra_steamapps);
                }
            }
        }

        for steamapps_path in lib_paths {
            if !steamapps_path.is_dir() {
                continue;
            }
            let apps = collect_app_ids(&steamapps_path);
            let lib = SteamLibrary {
                path: steamapps_path,
                apps,
            };
            libraries.push(lib);
        }
    }

    libraries
}

fn collect_app_ids(steamapps_dir: &PathBuf) -> Vec<u32> {
    let mut ids = Vec::new();
    if let Ok(entries) = std::fs::read_dir(steamapps_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("appmanifest_") && name_str.ends_with(".acf") {
                let id_str = name_str
                    .strip_prefix("appmanifest_")
                    .and_then(|s| s.strip_suffix(".acf"))
                    .unwrap_or("");
                if let Ok(id) = id_str.parse::<u32>() {
                    ids.push(id);
                }
            }
        }
    }
    ids
}

fn parse_library_folders_vdf(contents: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut in_entry = false;
    let mut depth = 0i32;

    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed == "{" {
            depth += 1;
            if depth == 2 {
                in_entry = true;
            }
        } else if trimmed == "}" {
            if depth == 2 {
                in_entry = false;
            }
            depth -= 1;
        } else if in_entry && depth == 2 {
            if let Some((key, value)) = parse_vdf_key_value(trimmed) {
                if key == "path" {
                    paths.push(PathBuf::from(value));
                }
            }
        }
    }

    paths
}

fn parse_vdf_key_value(line: &str) -> Option<(String, String)> {
    // Lines look like: "key"    "value"
    let line = line.trim();
    if !line.starts_with('"') {
        return None;
    }
    let rest = &line[1..];
    let key_end = rest.find('"')?;
    let key = rest[..key_end].to_string();
    let after_key = rest[key_end + 1..].trim();
    if !after_key.starts_with('"') {
        return None;
    }
    let val_rest = &after_key[1..];
    let val_end = val_rest.find('"')?;
    let value = val_rest[..val_end].to_string();
    Some((key, value))
}

fn parse_acf_install_dir(contents: &str) -> Option<String> {
    for line in contents.lines() {
        let trimmed = line.trim();
        if let Some((key, value)) = parse_vdf_key_value(trimmed) {
            if key == "installdir" {
                return Some(value);
            }
        }
    }
    None
}

pub fn find_game_path(app_id: u32) -> Option<PathBuf> {
    let libraries = find_steam_libraries();
    for lib in &libraries {
        let manifest_path = lib.path.join(format!("appmanifest_{app_id}.acf"));
        if manifest_path.exists() {
            if let Ok(contents) = std::fs::read_to_string(&manifest_path) {
                if let Some(install_dir) = parse_acf_install_dir(&contents) {
                    let game_path = lib.path.join("common").join(install_dir);
                    if game_path.is_dir() {
                        return Some(game_path);
                    }
                }
            }
        }
    }
    None
}

pub fn detect_games() -> Vec<(GameKind, PathBuf)> {
    let mut found = Vec::new();
    for kind in GameKind::all() {
        if let Some(app_id) = kind.steam_app_id() {
            if let Some(path) = find_game_path(app_id) {
                found.push((kind, path));
            }
        }
    }
    found
}

// ── Launch helpers ─────────────────────────────────────────────────────────

/// Return the root Steam installation directory, or `None` when Steam cannot
/// be found on this machine.
pub fn find_steam_root() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let candidates = [
        home.join(".steam").join("steam"),
        home.join(".local").join("share").join("Steam"),
        home.join("snap")
            .join("steam")
            .join("common")
            .join(".steam")
            .join("steam"),
        home.join(".var")
            .join("app")
            .join("com.valvesoftware.Steam")
            .join(".steam")
            .join("steam"),
    ];
    candidates.into_iter().find(|p| p.is_dir())
}

/// Return the Proton compatdata directory for `app_id`.
///
/// Searches the Steam library that contains the game's `appmanifest_<app_id>.acf`
/// first (which is where Steam always stores the matching `compatdata` entry),
/// then falls back to all other known Steam libraries.  This replaces the old
/// approach of checking a static list of hardcoded home-relative paths, which
/// failed whenever games were installed in a non-default Steam library.
pub fn find_compatdata_path(app_id: u32) -> Option<PathBuf> {
    let libraries = find_steam_libraries();

    // Prefer the library that actually contains the game's appmanifest — Steam
    // always creates the compatdata entry next to the appmanifest.
    for lib in &libraries {
        let manifest = lib.path.join(format!("appmanifest_{app_id}.acf"));
        if manifest.exists() {
            let path = lib.path.join("compatdata").join(app_id.to_string());
            if path.is_dir() {
                return Some(path);
            }
        }
    }

    // Fallback: check all libraries (handles e.g. compatdata created before the
    // game was moved to a different library).
    for lib in &libraries {
        let path = lib.path.join("compatdata").join(app_id.to_string());
        if path.is_dir() {
            return Some(path);
        }
    }

    None
}

/// Return the `proton` run-script from any Proton installation found in the
/// Steam libraries.  Prefers "Proton Experimental" over numbered releases
/// (it sorts last lexicographically), and among numbered releases picks the
/// highest folder name.  Note: "Proton 10.0" sorts before "Proton 9.0" with
/// plain string comparison; this edge-case is accepted given that Proton
/// Experimental is typically the most current choice anyway.
///
/// This is a generic fallback.  Prefer [`find_proton_for_app`] when you know
/// which app you are launching so that Steam's per-app Proton selection is
/// honoured.
pub fn find_proton_run() -> Option<PathBuf> {
    let libraries = find_steam_libraries();
    let mut candidates: Vec<PathBuf> = Vec::new();
    for lib in &libraries {
        let common = lib.path.join("common");
        if let Ok(entries) = std::fs::read_dir(&common) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                if name.to_string_lossy().starts_with("Proton") && entry.path().is_dir() {
                    let proton_script = entry.path().join("proton");
                    if proton_script.is_file() {
                        candidates.push(proton_script);
                    }
                }
            }
        }
    }
    // Sort to prefer higher version numbers (lexicographic order works for
    // "Proton 9.0", "Proton Experimental", etc.)
    candidates.sort();
    candidates.into_iter().last()
}

// ── Steam config.vdf helpers ──────────────────────────────────────────────

/// Parse a VDF line that is a bare section-header key with no associated
/// value, e.g. `"CompatToolMapping"`.  Returns `None` for lines that have
/// a value (handled by [`parse_vdf_key_value`]) or are not quoted strings.
fn parse_vdf_section_name(line: &str) -> Option<String> {
    let line = line.trim();
    if !line.starts_with('"') {
        return None;
    }
    let rest = &line[1..];
    let key_end = rest.find('"')?;
    let key = rest[..key_end].to_string();
    // Ensure nothing follows the closing quote (otherwise it is a key-value pair)
    if rest[key_end + 1..].trim().is_empty() {
        Some(key)
    } else {
        None
    }
}

/// Return the path to Steam's main `config/config.vdf`, trying all known
/// Steam installation roots.
fn find_config_vdf() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let candidates = [
        home.join(".steam").join("steam").join("config").join("config.vdf"),
        home.join(".local")
            .join("share")
            .join("Steam")
            .join("config")
            .join("config.vdf"),
        home.join("snap")
            .join("steam")
            .join("common")
            .join(".steam")
            .join("steam")
            .join("config")
            .join("config.vdf"),
        home.join(".var")
            .join("app")
            .join("com.valvesoftware.Steam")
            .join(".steam")
            .join("steam")
            .join("config")
            .join("config.vdf"),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

/// Parse `config.vdf` text and return the `CompatToolMapping > <app_id> > name`
/// value for `app_id`, if present.
///
/// The returned string is the tool identifier Steam stored, which may be a
/// short name (`proton_experimental`, `proton_9`) or a full directory name
/// (`Proton 9.0-3`).  Returns `None` when no mapping exists for the app.
pub fn parse_compat_tool_from_config_vdf(contents: &str, app_id: u32) -> Option<String> {
    let app_id_str = app_id.to_string();
    let mut depth: i32 = 0;

    #[derive(PartialEq)]
    enum State {
        LookingForCompatMapping,
        InCompatMapping,
        InApp,
    }

    let mut state = State::LookingForCompatMapping;
    let mut compat_depth: i32 = -1;
    let mut app_depth: i32 = -1;
    let mut pending_key: Option<String> = None;

    for line in contents.lines() {
        let trimmed = line.trim();

        if trimmed == "{" {
            depth += 1;
            match &state {
                State::LookingForCompatMapping => {
                    if pending_key
                        .as_deref()
                        .map(|k| k.eq_ignore_ascii_case("compattoolmapping"))
                        .unwrap_or(false)
                    {
                        state = State::InCompatMapping;
                        compat_depth = depth;
                    }
                }
                State::InCompatMapping => {
                    if pending_key.as_deref() == Some(app_id_str.as_str()) {
                        state = State::InApp;
                        app_depth = depth;
                    }
                }
                State::InApp => {}
            }
            pending_key = None;
        } else if trimmed == "}" {
            match &state {
                State::InApp if depth == app_depth => {
                    state = State::InCompatMapping;
                    app_depth = -1;
                }
                State::InCompatMapping if depth == compat_depth => {
                    state = State::LookingForCompatMapping;
                    compat_depth = -1;
                }
                _ => {}
            }
            depth -= 1;
            pending_key = None;
        } else if let Some((key, value)) = parse_vdf_key_value(trimmed) {
            if state == State::InApp && key.eq_ignore_ascii_case("name") && !value.is_empty() {
                return Some(value);
            }
            pending_key = None;
        } else if let Some(section_name) = parse_vdf_section_name(trimmed) {
            pending_key = Some(section_name);
        } else {
            pending_key = None;
        }
    }

    None
}

/// Convert a Proton tool identifier read from `config.vdf` into a directory
/// name prefix used to locate the Proton installation folder.
///
/// Examples:
/// * `"proton_experimental"` → `"Proton Experimental"`
/// * `"proton_9"`            → `"Proton 9"`
/// * `"Proton 9.0-3"`        → `"Proton 9.0-3"` (already a full name)
fn tool_name_to_dir_prefix(tool_name: &str) -> String {
    if tool_name.contains(' ') {
        // Already looks like a full directory name (e.g. "Proton 9.0-3").
        tool_name.to_string()
    } else {
        // Convert underscore-separated identifier to title-cased, space-
        // separated prefix, e.g. "proton_experimental" → "Proton Experimental".
        tool_name
            .split('_')
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    None => String::new(),
                    Some(first) => first.to_uppercase().to_string() + chars.as_str(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Return the `proton` run-script for the Proton version that Steam has
/// configured for `app_id`.
///
/// Reads `~/.steam/steam/config/config.vdf` to determine which Proton tool
/// Steam assigned to the game, then locates that tool in the Steam libraries.
/// Falls back to [`find_proton_run`] (highest available version) when:
/// * `config.vdf` cannot be read or contains no mapping for `app_id`, or
/// * the configured Proton version is not installed.
pub fn find_proton_for_app(app_id: u32) -> Option<PathBuf> {
    let libraries = find_steam_libraries();

    if let Some(tool_name) = find_config_vdf()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|contents| parse_compat_tool_from_config_vdf(&contents, app_id))
    {
        if !tool_name.is_empty() {
            let dir_prefix = tool_name_to_dir_prefix(&tool_name);
            let mut candidates: Vec<PathBuf> = Vec::new();

            for lib in &libraries {
                let common = lib.path.join("common");
                if let Ok(entries) = std::fs::read_dir(&common) {
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        // Accept an exact match or a directory whose name starts
                        // with the prefix (handles versioned suffixes like "-3").
                        if name == dir_prefix || name.starts_with(&(dir_prefix.clone() + " ")) {
                            let proton_script = entry.path().join("proton");
                            if proton_script.is_file() {
                                candidates.push(proton_script);
                            }
                        }
                    }
                }
            }

            if !candidates.is_empty() {
                candidates.sort();
                return candidates.into_iter().last();
            }
        }
    }

    // Fallback: use the highest available Proton version.
    find_proton_run()
}

/// Launch a game executable.
///
/// * The **primary** game binary (first entry in
///   [`GameKind::known_executables`]) is launched through the Steam URL
///   scheme (`steam://run/<app_id>`) so that Steam is properly notified:
///   the overlay is active, playtime is tracked, and cloud-saves sync as
///   expected.
///
/// * **Script-extender loaders** and other secondary executables (SKSE,
///   F4SE, NVSE, …) are launched directly via Proton because
///   `steam://run/<app_id>` always starts the game's default executable —
///   it does not respect which binary the user has selected.  Running via
///   Proton ensures the selected binary actually starts inside the correct
///   Wine environment.
///
/// Required environment variables for Proton launches (set automatically):
/// * `STEAM_COMPAT_DATA_PATH` — the game's per-app Wine prefix as configured
///   by Steam (`compatdata/<app_id>` in the Steam library that holds the
///   game).
/// * `STEAM_COMPAT_CLIENT_INSTALL_PATH` — Steam installation root, used by
///   Proton to locate its own runtime libraries and scripts.
///
/// The Proton binary used is the one Steam has configured for this specific
/// game (read from `config/config.vdf`), falling back to the highest
/// available Proton version when no per-app mapping exists.
///
/// Returns `Ok(())` if the process was successfully spawned, or an error
/// message string on failure.
pub fn launch_game_executable(
    game: &crate::core::games::Game,
    exe_name: &str,
) -> Result<(), String> {
    let exe_path = game.root_path.join(exe_name);
    if !exe_path.is_file() {
        return Err(format!("Executable not found: {}", exe_path.display()));
    }

    let app_id = game
        .kind
        .steam_app_id()
        .ok_or_else(|| "Game has no Steam App ID".to_string())?;

    // Determine whether this is the primary game executable.  Script-extender
    // loaders (skse64_loader.exe, f4se_loader.exe, …) are NOT the primary exe
    // and must be launched via Proton so the selected binary actually runs.
    let is_primary = game
        .kind
        .known_executables()
        .first()
        .map(|&primary| primary.eq_ignore_ascii_case(exe_name))
        .unwrap_or(false);

    if is_primary {
        // Use the Steam URL scheme so that the game starts through Steam,
        // enabling the overlay, playtime tracking, and cloud-save sync.
        std::process::Command::new("xdg-open")
            .arg(format!("steam://run/{app_id}"))
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Failed to launch {exe_name} via Steam: {e}"))
    } else {
        // Run the selected executable (e.g. SKSE, F4SE) via Proton directly.
        // steam://run/<app_id> would always launch the game's default exe and
        // would ignore the user's choice, so Proton is the correct path here.
        // Use the Proton version that Steam has configured for this specific
        // game (read from config.vdf), falling back to the highest available.
        let proton = find_proton_for_app(app_id)
            .ok_or_else(|| "No Proton installation found in Steam libraries".to_string())?;

        let steam_root = find_steam_root()
            .ok_or_else(|| "Steam installation not found".to_string())?;

        let compatdata = find_compatdata_path(app_id)
            .ok_or_else(|| format!("Steam compatdata not found for app {app_id}"))?;

        std::process::Command::new(&proton)
            .arg("run")
            .arg(&exe_path)
            // STEAM_COMPAT_DATA_PATH points Proton to the game's Wine prefix
            // (the per-app compatdata directory created by Steam).
            // STEAM_COMPAT_CLIENT_INSTALL_PATH tells Proton where the Steam
            // installation root is so it can locate its own runtime files.
            .env("STEAM_COMPAT_DATA_PATH", &compatdata)
            .env("STEAM_COMPAT_CLIENT_INSTALL_PATH", &steam_root)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Failed to launch {exe_name} via Proton: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::games::{Game, GameKind};

    /// Returns `true` when `exe_name` is the primary executable for `game`
    /// (i.e. the launch path would use `xdg-open steam://run/…`).
    fn is_primary_exe(game: &Game, exe_name: &str) -> bool {
        game.kind
            .known_executables()
            .first()
            .map(|&primary| primary.eq_ignore_ascii_case(exe_name))
            .unwrap_or(false)
    }

    #[test]
    fn primary_exe_is_identified_correctly() {
        let game = Game::new(GameKind::SkyrimSE, std::path::PathBuf::from("/fake/path"));
        assert!(is_primary_exe(&game, "SkyrimSE.exe"));
        assert!(is_primary_exe(&game, "skyrimse.exe")); // case-insensitive
        assert!(!is_primary_exe(&game, "skse64_loader.exe"));
        assert!(!is_primary_exe(&game, "SkyrimSELauncher.exe"));
    }

    #[test]
    fn script_extender_is_not_primary_for_any_game() {
        let loaders = [
            (GameKind::SkyrimSE, "skse64_loader.exe"),
            (GameKind::SkyrimLE, "skse_loader.exe"),
            (GameKind::Fallout4, "f4se_loader.exe"),
            (GameKind::Fallout3, "fose_loader.exe"),
            (GameKind::FalloutNV, "nvse_loader.exe"),
            (GameKind::Oblivion, "obse_loader.exe"),
        ];
        for (kind, loader) in loaders {
            let game = Game::new(kind.clone(), std::path::PathBuf::from("/fake/path"));
            assert!(
                !is_primary_exe(&game, loader),
                "{loader} should not be the primary exe for {kind:?}"
            );
        }
    }

    #[test]
    fn launch_game_executable_fails_when_exe_not_found() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let game = Game::new(GameKind::SkyrimSE, tmp.path().to_path_buf());
        let result = launch_game_executable(&game, "SkyrimSE.exe");
        assert!(result.is_err(), "should fail when exe does not exist on disk");
        assert!(
            result.unwrap_err().contains("Executable not found"),
            "error should mention the missing file"
        );
    }

    #[test]
    fn launch_game_executable_fails_for_script_extender_when_not_found() {
        // Script extenders are launched via Proton (not steam://).
        // The executable-existence check is performed before any Proton/Steam
        // lookup, so a missing loader still returns an error immediately.
        let tmp = tempfile::tempdir().expect("tempdir");
        let game = Game::new(GameKind::SkyrimSE, tmp.path().to_path_buf());
        let result = launch_game_executable(&game, "skse64_loader.exe");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Executable not found"));
    }
}
