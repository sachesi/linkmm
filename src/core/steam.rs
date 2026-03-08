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

/// Return the Proton compatdata directory for `app_id`, searching all known
/// Steam library locations.
pub fn find_compatdata_path(app_id: u32) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let roots: &[&str] = &[
        ".steam/steam/steamapps/compatdata",
        ".local/share/Steam/steamapps/compatdata",
        "snap/steam/common/.steam/steam/steamapps/compatdata",
        ".var/app/com.valvesoftware.Steam/.steam/steam/steamapps/compatdata",
    ];
    for root in roots {
        let path = home.join(root).join(app_id.to_string());
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

/// Launch a game executable via Proton.
///
/// Every executable — the primary game binary as well as launchers and
/// script-extender loaders (SKSE, F4SE, NVSE, …) — is run through Proton so
/// that the correct Wine prefix and Steam runtime are always used.
///
/// Required environment variables set automatically:
/// * `STEAM_COMPAT_DATA_PATH` — path to the game's per-app Wine prefix
///   (the `compatdata/<app_id>` directory managed by Steam).
/// * `STEAM_COMPAT_CLIENT_INSTALL_PATH` — Steam installation root, used by
///   Proton to locate its own runtime libraries and scripts.
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

    let proton = find_proton_run()
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
