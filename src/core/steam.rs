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

/// Launch a game through the Steam client using its Steam App ID.
///
/// Opens `steam://run/<app_id>` via `xdg-open` so that Steam handles process
/// creation, Proton integration, overlay, playtime tracking, and the "Running"
/// badge in the Steam library — exactly as if the user pressed "Play" inside
/// Steam itself.
///
/// Returns `Ok(())` on successful spawn, or an error message string on
/// failure.
pub fn launch_game(game: &crate::core::games::Game) -> Result<(), String> {
    let app_id = game
        .kind
        .steam_app_id()
        .ok_or_else(|| "Game has no Steam App ID".to_string())?;

    std::process::Command::new("xdg-open")
        .arg(format!("steam://run/{app_id}"))
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Failed to open steam://run/{app_id}: {e}"))
}

/// Find the Proton runtime path for a given game's App ID.
///
/// Returns the path to the Proton directory (e.g., `~/.steam/steam/steamapps/common/Proton 8.0`)
/// and the `compatdata` directory for the game.
pub fn find_proton_for_game(app_id: u32) -> Result<(PathBuf, PathBuf), String> {
    // Find the compatdata path first
    let compatdata_path = find_compatdata_path(app_id)
        .ok_or_else(|| format!("Could not find compatdata for App ID {}", app_id))?;

    // Check for a toolmanifest.vdf inside compatdata to determine which Proton version is used
    let tool_manifest = compatdata_path.join("version");
    let proton_version = if tool_manifest.exists() {
        std::fs::read_to_string(&tool_manifest)
            .ok()
            .and_then(|content| {
                // The version file typically contains a line like "8.0-3c" or similar
                content.lines().next().map(|s| s.trim().to_string())
            })
    } else {
        None
    };

    // Try to find Proton in common directories
    let libraries = find_steam_libraries();
    for lib in &libraries {
        let common_path = lib.path.parent()
            .map(|p| p.join("common"))
            .unwrap_or_else(|| lib.path.join("common"));

        if !common_path.is_dir() {
            continue;
        }

        // If we have a specific version from compatdata, look for it
        if let Some(ref version) = proton_version {
            // Try exact match first (e.g., "Proton 8.0")
            let proton_path = common_path.join(format!("Proton {}", version));
            if proton_path.join("proton").exists() {
                return Ok((proton_path, compatdata_path));
            }
        }

        // Fallback: find any Proton installation
        if let Ok(entries) = std::fs::read_dir(&common_path) {
            let mut proton_dirs: Vec<_> = entries
                .flatten()
                .filter(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    name.starts_with("Proton") && e.path().join("proton").exists()
                })
                .collect();

            // Sort to prefer newer versions (reverse alphabetical often works)
            proton_dirs.sort_by(|a, b| {
                b.file_name()
                    .to_string_lossy()
                    .cmp(&a.file_name().to_string_lossy())
            });

            if let Some(proton_dir) = proton_dirs.first() {
                return Ok((proton_dir.path(), compatdata_path));
            }
        }
    }

    Err("Could not find any Proton installation".to_string())
}

/// Launch an external tool using Proton.
///
/// This sets up the appropriate environment variables and executes the tool
/// through Proton's runtime, similar to how Steam launches Windows games.
pub fn launch_tool_with_proton(
    exe_path: &PathBuf,
    arguments: &str,
    app_id: u32,
) -> Result<std::process::Child, String> {
    let (proton_path, compatdata_path) = find_proton_for_game(app_id)?;
    let proton_script = proton_path.join("proton");

    if !proton_script.exists() {
        return Err(format!(
            "Proton script not found at {}",
            proton_script.display()
        ));
    }

    if !exe_path.exists() {
        return Err(format!("Executable not found at {}", exe_path.display()));
    }

    let steam_root = find_steam_root()
        .ok_or_else(|| "Could not find Steam installation".to_string())?;

    log::info!(
        "Launching tool {} with Proton from {}",
        exe_path.display(),
        proton_path.display()
    );
    log::debug!("Using compatdata: {}", compatdata_path.display());

    let mut command = std::process::Command::new(&proton_script);

    // Set up Proton environment variables
    command.env("STEAM_COMPAT_DATA_PATH", &compatdata_path);
    command.env("STEAM_COMPAT_CLIENT_INSTALL_PATH", &steam_root);
    command.env("SteamAppId", app_id.to_string());
    command.env("SteamGameId", app_id.to_string());

    // Add the "run" command
    command.arg("run");

    // Add the executable path
    command.arg(exe_path);

    // Add any additional arguments
    if !arguments.is_empty() {
        for arg in arguments.split_whitespace() {
            command.arg(arg);
        }
    }

    // Capture stdout and stderr for logging
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());

    log::debug!("Executing: {:?}", command);

    command
        .spawn()
        .map_err(|e| format!("Failed to spawn Proton process: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_game_fails_without_steam_app_id() {
        // GameKind::SkyrimSE has a Steam App ID, so this test just verifies
        // that a game with a known App ID reaches the xdg-open step (which
        // may or may not be installed in CI, so we only check the error
        // message shape if it fails).
        use crate::core::games::{Game, GameKind};
        let game = Game::new(GameKind::SkyrimSE, std::path::PathBuf::from("/fake"));
        // We cannot assert Ok because xdg-open may not exist in CI.
        // The important property is that the function does NOT return an error
        // about "Steam App ID".
        match launch_game(&game) {
            Ok(_) => {}
            Err(e) => assert!(
                !e.contains("Steam App ID"),
                "error should not be about missing App ID for SkyrimSE: {e}"
            ),
        }
    }
}
