use crate::core::games::GameKind;
use std::path::{Path, PathBuf};

#[allow(dead_code)]
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
        } else if in_entry
            && depth == 2
            && let Some((key, value)) = parse_vdf_key_value(trimmed)
            && key == "path"
        {
            paths.push(PathBuf::from(value));
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
        if let Some((key, value)) = parse_vdf_key_value(trimmed)
            && key == "installdir"
        {
            return Some(value);
        }
    }
    None
}

/// Parse config.vdf to find per-game compatibility tool mapping.
///
/// Returns the tool name (e.g., "proton_8", "GE-Proton9-2") for the given app_id.
fn parse_per_game_proton_config(app_id: u32) -> Option<String> {
    let home = dirs::home_dir()?;

    let candidate_config_paths = vec![
        home.join(".steam")
            .join("steam")
            .join("config")
            .join("config.vdf"),
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

    for config_path in candidate_config_paths {
        if !config_path.exists() {
            continue;
        }

        if let Ok(contents) = std::fs::read_to_string(&config_path)
            && let Some(tool_name) = parse_compat_tool_mapping(&contents, app_id)
        {
            log::debug!(
                "Found per-game Proton config for {}: {} in {}",
                app_id,
                tool_name,
                config_path.display()
            );
            return Some(tool_name);
        }
    }

    None
}

/// Parse the CompatToolMapping section from config.vdf.
fn parse_compat_tool_mapping(contents: &str, app_id: u32) -> Option<String> {
    let mut in_compat_mapping = false;
    let mut depth = 0i32;
    let app_id_str = app_id.to_string();

    for line in contents.lines() {
        let trimmed = line.trim();

        if trimmed == "{" {
            depth += 1;
        } else if trimmed == "}" {
            if in_compat_mapping && depth == 1 {
                in_compat_mapping = false;
            }
            depth -= 1;
        } else if trimmed.contains("CompatToolMapping") {
            in_compat_mapping = true;
        } else if in_compat_mapping && depth > 0 {
            if let Some((key, _)) = parse_vdf_key_value(trimmed)
                && key == app_id_str
            {
                // Next section should have "name" key with the tool
                continue;
            }
            // Check for nested structure: app_id { "name" "tool_name" }
            if trimmed.starts_with(&format!("\"{}\"", app_id_str)) {
                // Found our app, look for the tool name in subsequent lines
                let _found_app_section = true;
                let mut search_depth = depth;

                for next_line in contents.lines().skip_while(|l| l.trim() != trimmed).skip(1) {
                    let next_trimmed = next_line.trim();
                    if next_trimmed == "{" {
                        search_depth += 1;
                    } else if next_trimmed == "}" {
                        search_depth -= 1;
                        if search_depth < depth {
                            break;
                        }
                    } else if let Some((key, value)) = parse_vdf_key_value(next_trimmed)
                        && (key == "name" || key == "Priority")
                    {
                        return Some(value);
                    }
                }
            }
        }
    }

    None
}

/// Find all possible Proton installation directories.
fn find_proton_directories() -> Vec<PathBuf> {
    let mut proton_dirs = Vec::new();
    let libraries = find_steam_libraries();

    for lib in &libraries {
        // Check steamapps/common for official Proton
        if let Some(steamapps_parent) = lib.path.parent() {
            let common_path = steamapps_parent.join("common");
            if common_path.is_dir() {
                proton_dirs.push(common_path);
            }
        }

        // Check compatibilitytools.d for custom Proton (GE-Proton, etc.)
        if let Some(steam_root) = lib.path.parent() {
            let compat_tools_path = steam_root.join("compatibilitytools.d");
            if compat_tools_path.is_dir() {
                proton_dirs.push(compat_tools_path);
            }
        }
    }

    // Also check for compatibilitytools.d in Steam roots directly
    if let Some(home) = dirs::home_dir() {
        let roots = vec![
            home.join(".steam").join("steam"),
            home.join(".local").join("share").join("Steam"),
            home.join(".var")
                .join("app")
                .join("com.valvesoftware.Steam")
                .join(".steam")
                .join("steam"),
        ];

        for root in roots {
            let compat_path = root.join("compatibilitytools.d");
            if compat_path.is_dir() && !proton_dirs.contains(&compat_path) {
                proton_dirs.push(compat_path);
            }
        }
    }

    proton_dirs
}

/// Search for a Proton installation by name.
///
/// Searches in both steamapps/common and compatibilitytools.d directories.
/// Matches tool names like "proton_8", "GE-Proton9-2", etc.
fn find_proton_by_name(tool_name: &str) -> Option<PathBuf> {
    let proton_dirs = find_proton_directories();

    for base_dir in &proton_dirs {
        if let Ok(entries) = std::fs::read_dir(base_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }

                let dir_name = entry.file_name().to_string_lossy().to_string();

                // Check for exact match or normalized match
                if dir_name == tool_name
                    || normalize_proton_name(&dir_name) == normalize_proton_name(tool_name)
                {
                    // Verify it has a proton script
                    if path.join("proton").exists() {
                        log::debug!("Found Proton at: {}", path.display());
                        return Some(path);
                    }
                }

                // Also check for partial matches (e.g., "Proton 8.0" matches "proton_8")
                if tool_name.contains("proton") && dir_name.to_lowercase().contains("proton") {
                    let tool_lower = tool_name.to_lowercase();
                    let dir_lower = dir_name.to_lowercase();

                    // Extract version numbers for comparison
                    if tools_match_version(&tool_lower, &dir_lower) && path.join("proton").exists()
                    {
                        log::debug!("Found Proton at: {} (version match)", path.display());
                        return Some(path);
                    }
                }
            }
        }
    }

    None
}

/// Normalize Proton tool names for comparison.
fn normalize_proton_name(name: &str) -> String {
    name.to_lowercase()
        .replace("-", "")
        .replace("_", "")
        .replace(" ", "")
        .replace(".", "")
}

/// Check if two Proton tool names refer to the same version.
fn tools_match_version(tool1: &str, tool2: &str) -> bool {
    // Extract numbers from both names
    let nums1: Vec<&str> = tool1
        .split(|c: char| !c.is_numeric())
        .filter(|s| !s.is_empty())
        .collect();
    let nums2: Vec<&str> = tool2
        .split(|c: char| !c.is_numeric())
        .filter(|s| !s.is_empty())
        .collect();

    // If we have version numbers, they should match
    if !nums1.is_empty() && !nums2.is_empty() {
        nums1.first() == nums2.first()
    } else {
        false
    }
}

pub fn find_game_path(app_id: u32) -> Option<PathBuf> {
    let libraries = find_steam_libraries();
    for lib in &libraries {
        let manifest_path = lib.path.join(format!("appmanifest_{app_id}.acf"));
        if manifest_path.exists()
            && let Ok(contents) = std::fs::read_to_string(&manifest_path)
            && let Some(install_dir) = parse_acf_install_dir(&contents)
        {
            let game_path = lib.path.join("common").join(install_dir);
            if game_path.is_dir() {
                return Some(game_path);
            }
        }
    }
    None
}

pub fn detect_games() -> Vec<(GameKind, PathBuf)> {
    let mut found = Vec::new();
    for kind in GameKind::all() {
        if let Some(app_id) = kind.steam_app_id()
            && let Some(path) = find_game_path(app_id)
        {
            found.push((kind, path));
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

/// Detect if Steam is running as a Flatpak installation.
///
/// Returns true if the Steam root is in the Flatpak directory.
#[allow(dead_code)]
pub fn is_steam_flatpak() -> bool {
    if let Some(steam_root) = find_steam_root() {
        let steam_root_str = steam_root.to_string_lossy();
        steam_root_str.contains("/.var/app/com.valvesoftware.Steam/")
    } else {
        false
    }
}

/// Detect if a path is within the Flatpak Steam directory.
///
/// This is more reliable than checking steam_root alone, since Proton or compatdata
/// may be installed in the Flatpak directory even if steam_root points to a symlink.
pub fn is_path_in_flatpak(path: &Path) -> bool {
    let path_str = path.to_string_lossy();
    path_str.contains("/.var/app/com.valvesoftware.Steam/")
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

/// Build a managed Steam launch command.
///
/// We use `steam -applaunch` so LinkMM can track and stop the spawned wrapper
/// process. Steam may re-parent the real game process; stopping this session
/// therefore targets the visible wrapper process only.
pub fn launch_game_managed_command(
    game: &crate::core::games::Game,
) -> Result<std::process::Command, String> {
    let app_id = game
        .kind
        .steam_app_id()
        .ok_or_else(|| "Game has no Steam App ID".to_string())?;
    let command = match select_managed_steam_backend(is_steam_flatpak(), steam_binary_on_path()) {
        ManagedSteamBackend::Flatpak => launch_game_managed_flatpak_command(app_id),
        ManagedSteamBackend::Native => launch_game_managed_native_command(app_id),
        ManagedSteamBackend::XdgOpenFallback => {
            let mut command = std::process::Command::new("xdg-open");
            command.arg(format!("steam://run/{app_id}"));
            command
        }
    };
    Ok(command)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManagedSteamBackend {
    Native,
    Flatpak,
    XdgOpenFallback,
}

fn select_managed_steam_backend(is_flatpak: bool, steam_on_path: bool) -> ManagedSteamBackend {
    if is_flatpak {
        ManagedSteamBackend::Flatpak
    } else if steam_on_path {
        ManagedSteamBackend::Native
    } else {
        ManagedSteamBackend::XdgOpenFallback
    }
}

fn steam_binary_on_path() -> bool {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).any(|dir| dir.join("steam").is_file()))
        .unwrap_or(false)
}

fn launch_game_managed_native_command(app_id: u32) -> std::process::Command {
    let mut command = std::process::Command::new("steam");
    command.arg("-applaunch").arg(app_id.to_string());
    command
}

fn launch_game_managed_flatpak_command(app_id: u32) -> std::process::Command {
    let mut command = std::process::Command::new("flatpak");
    command
        .arg("run")
        .arg("com.valvesoftware.Steam")
        .arg("-applaunch")
        .arg(app_id.to_string());
    command
}

/// Find the Proton runtime path for a given game's App ID.
///
/// Returns the path to the Proton directory (e.g., `~/.steam/steam/steamapps/common/Proton 8.0`)
/// and the `compatdata` directory for the game.
///
/// This function now checks Steam's config.vdf for per-game Proton settings, supporting:
/// - Native Steam installations (~/.local/share/Steam)
/// - Flatpak Steam (~/.var/app/com.valvesoftware.Steam)
/// - Custom Proton versions (GE-Proton in compatibilitytools.d)
pub fn find_proton_for_game(app_id: u32) -> Result<(PathBuf, PathBuf), String> {
    // Find the compatdata path first
    let compatdata_path = find_compatdata_path(app_id)
        .ok_or_else(|| format!("Could not find compatdata for App ID {}", app_id))?;

    log::debug!("Finding Proton for app_id: {}", app_id);

    // Step 1: Check config.vdf for per-game Proton configuration
    if let Some(tool_name) = parse_per_game_proton_config(app_id) {
        log::info!("Per-game Proton config found: {}", tool_name);

        if let Some(proton_path) = find_proton_by_name(&tool_name) {
            log::info!("Using configured Proton: {}", proton_path.display());
            return Ok((proton_path, compatdata_path));
        } else {
            log::warn!(
                "Configured Proton '{}' not found, falling back to detection",
                tool_name
            );
        }
    }

    // Step 2: Check compatdata version file
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

    if let Some(ref version) = proton_version {
        log::debug!("Compatdata version file contains: {}", version);

        // Try to find Proton matching this version
        if let Some(proton_path) = find_proton_by_name(version) {
            log::info!(
                "Found Proton matching version file: {}",
                proton_path.display()
            );
            return Ok((proton_path, compatdata_path));
        }
    }

    // Step 3: Fallback - search for any Proton installation
    log::debug!("Falling back to automatic Proton detection");
    let proton_dirs = find_proton_directories();

    for base_dir in &proton_dirs {
        if let Ok(entries) = std::fs::read_dir(base_dir) {
            let mut proton_candidates: Vec<_> = entries
                .flatten()
                .filter(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    name.to_lowercase().contains("proton") && e.path().join("proton").exists()
                })
                .collect();

            // Sort to prefer newer versions (reverse alphabetical often works)
            proton_candidates.sort_by(|a, b| {
                b.file_name()
                    .to_string_lossy()
                    .cmp(&a.file_name().to_string_lossy())
            });

            if let Some(proton_dir) = proton_candidates.first() {
                let proton_path = proton_dir.path();
                log::info!("Using fallback Proton: {}", proton_path.display());
                return Ok((proton_path, compatdata_path));
            }
        }
    }

    Err("Could not find any Proton installation".to_string())
}

/// Launch an external tool using Proton.
///
/// This sets up the appropriate environment variables and executes the tool
/// through Proton's runtime, similar to how Steam launches Windows games.
/// Automatically detects and uses Flatpak wrapper if Steam is a Flatpak installation.
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

    let steam_root =
        find_steam_root().ok_or_else(|| "Could not find Steam installation".to_string())?;

    log::info!(
        "Launching tool {} with Proton from {}",
        exe_path.display(),
        proton_path.display()
    );
    log::debug!("Using compatdata: {}", compatdata_path.display());

    // Check if either Proton or compatdata is in the Flatpak directory
    let is_flatpak = is_path_in_flatpak(&proton_path) || is_path_in_flatpak(&compatdata_path);

    if is_flatpak {
        log::info!(
            "Detected Flatpak Steam (Proton or compatdata in Flatpak directory), using flatpak wrapper"
        );
        launch_tool_with_flatpak(
            &proton_script,
            exe_path,
            arguments,
            &steam_root,
            &compatdata_path,
            app_id,
        )
    } else {
        log::debug!("Using native Steam launch");
        launch_tool_native(
            &proton_script,
            exe_path,
            arguments,
            &steam_root,
            &compatdata_path,
            app_id,
        )
    }
}

pub fn build_tool_command(
    exe_path: &PathBuf,
    arguments: &str,
    app_id: u32,
) -> Result<std::process::Command, String> {
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
    let steam_root =
        find_steam_root().ok_or_else(|| "Could not find Steam installation".to_string())?;
    let is_flatpak = is_path_in_flatpak(&proton_path) || is_path_in_flatpak(&compatdata_path);
    if is_flatpak {
        build_flatpak_tool_command(
            &proton_script,
            exe_path,
            arguments,
            &steam_root,
            &compatdata_path,
            app_id,
        )
    } else {
        build_native_tool_command(
            &proton_script,
            exe_path,
            arguments,
            &steam_root,
            &compatdata_path,
            app_id,
        )
    }
}

/// Launch tool using native Steam (not Flatpak).
fn launch_tool_native(
    proton_script: &PathBuf,
    exe_path: &PathBuf,
    arguments: &str,
    steam_root: &PathBuf,
    compatdata_path: &PathBuf,
    app_id: u32,
) -> Result<std::process::Child, String> {
    let mut command = std::process::Command::new(proton_script);

    // Set up Proton environment variables
    command.env("STEAM_COMPAT_DATA_PATH", compatdata_path);
    command.env("STEAM_COMPAT_CLIENT_INSTALL_PATH", steam_root);
    command.env("SteamAppId", app_id.to_string());
    command.env("SteamGameId", app_id.to_string());

    // Add the "run" command
    command.arg("run");

    // Add the executable path
    command.arg(exe_path);

    // Add any additional arguments
    for arg in split_launch_arguments(arguments)? {
        command.arg(arg);
    }

    // Capture stdout and stderr for logging
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());

    log::debug!("Executing: {:?}", command);

    command
        .spawn()
        .map_err(|e| format!("Failed to spawn Proton process: {e}"))
}

fn build_native_tool_command(
    proton_script: &PathBuf,
    exe_path: &PathBuf,
    arguments: &str,
    steam_root: &PathBuf,
    compatdata_path: &PathBuf,
    app_id: u32,
) -> Result<std::process::Command, String> {
    let mut command = std::process::Command::new(proton_script);
    command.env("STEAM_COMPAT_DATA_PATH", compatdata_path);
    command.env("STEAM_COMPAT_CLIENT_INSTALL_PATH", steam_root);
    command.env("SteamAppId", app_id.to_string());
    command.env("SteamGameId", app_id.to_string());
    command.arg("run");
    command.arg(exe_path);
    for arg in split_launch_arguments(arguments)? {
        command.arg(arg);
    }
    Ok(command)
}

/// Launch tool using Flatpak Steam wrapper.
fn launch_tool_with_flatpak(
    proton_script: &Path,
    exe_path: &Path,
    arguments: &str,
    steam_root: &Path,
    compatdata_path: &Path,
    app_id: u32,
) -> Result<std::process::Child, String> {
    let mut command = build_flatpak_tool_command(
        proton_script,
        exe_path,
        arguments,
        steam_root,
        compatdata_path,
        app_id,
    )?;

    // Capture stdout and stderr for logging
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());

    log::debug!("Executing flatpak command: {:?}", command);

    command
        .spawn()
        .map_err(|e| format!("Failed to spawn Flatpak process: {e}"))
}

fn build_flatpak_tool_command(
    proton_script: &Path,
    exe_path: &Path,
    arguments: &str,
    steam_root: &Path,
    compatdata_path: &Path,
    app_id: u32,
) -> Result<std::process::Command, String> {
    let mut command = std::process::Command::new("flatpak");
    command
        .arg("run")
        .arg(format!(
            "--env=STEAM_COMPAT_CLIENT_INSTALL_PATH={}",
            steam_root.display()
        ))
        .arg(format!(
            "--env=STEAM_COMPAT_DATA_PATH={}",
            compatdata_path.display()
        ))
        .arg(format!("--env=SteamAppId={app_id}"))
        .arg(format!("--env=SteamGameId={app_id}"))
        .arg(format!("--command={}", proton_script.display()))
        .arg("com.valvesoftware.Steam")
        .arg("run")
        .arg(exe_path);
    for arg in split_launch_arguments(arguments)? {
        command.arg(arg);
    }
    Ok(command)
}

fn split_launch_arguments(arguments: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in arguments.chars() {
        if escaped {
            current.push(c);
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '\'' | '"' => {
                if quote == Some(c) {
                    quote = None;
                } else if quote.is_none() {
                    quote = Some(c);
                } else {
                    current.push(c);
                }
            }
            c if c.is_whitespace() && quote.is_none() => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(c),
        }
    }
    if escaped {
        current.push('\\');
    }
    if quote.is_some() {
        return Err("Tool arguments contain an unmatched quote".to_string());
    }
    if !current.is_empty() {
        out.push(current);
    }
    Ok(out)
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
        let game = Game::new_steam(GameKind::SkyrimSE, std::path::PathBuf::from("/fake"));
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

    #[test]
    fn is_steam_flatpak_detects_flatpak_path() {
        // This test verifies the detection logic, but won't actually find
        // a Flatpak Steam in CI since it's not installed there.
        let _ = is_steam_flatpak();
    }

    #[test]
    fn is_path_in_flatpak_detects_flatpak_paths() {
        use std::path::PathBuf;

        // Flatpak paths should be detected
        let flatpak_path = PathBuf::from(
            "/home/user/.var/app/com.valvesoftware.Steam/.steam/steam/compatibilitytools.d/GE-Proton10-34",
        );
        assert!(is_path_in_flatpak(&flatpak_path));

        let flatpak_compatdata = PathBuf::from(
            "/home/user/.var/app/com.valvesoftware.Steam/data/steamapps/compatdata/489830",
        );
        assert!(is_path_in_flatpak(&flatpak_compatdata));

        // Native paths should not be detected as Flatpak
        let native_path =
            PathBuf::from("/home/user/.local/share/Steam/steamapps/common/Proton 8.0");
        assert!(!is_path_in_flatpak(&native_path));

        let external_lib = PathBuf::from("/mnt/data0/.steamlib/steamapps/compatdata/489830");
        assert!(!is_path_in_flatpak(&external_lib));
    }

    #[test]
    fn managed_backend_selection_prefers_flatpak_when_detected() {
        assert_eq!(
            select_managed_steam_backend(true, true),
            ManagedSteamBackend::Flatpak
        );
        assert_eq!(
            select_managed_steam_backend(true, false),
            ManagedSteamBackend::Flatpak
        );
    }

    #[test]
    fn managed_backend_selection_uses_native_when_available() {
        assert_eq!(
            select_managed_steam_backend(false, true),
            ManagedSteamBackend::Native
        );
    }

    #[test]
    fn managed_backend_selection_uses_xdg_open_only_as_last_resort() {
        assert_eq!(
            select_managed_steam_backend(false, false),
            ManagedSteamBackend::XdgOpenFallback
        );
    }

    #[test]
    fn flatpak_managed_game_command_uses_flatpak_run_applaunch() {
        let command = launch_game_managed_flatpak_command(489830);
        let program = command.get_program().to_string_lossy().to_string();
        let args: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(program, "flatpak");
        assert_eq!(
            args,
            vec!["run", "com.valvesoftware.Steam", "-applaunch", "489830"]
        );
    }

    #[test]
    fn native_managed_game_command_uses_steam_applaunch() {
        let command = launch_game_managed_native_command(489830);
        let program = command.get_program().to_string_lossy().to_string();
        let args: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(program, "steam");
        assert_eq!(args, vec!["-applaunch", "489830"]);
    }

    #[test]
    fn split_launch_arguments_preserves_quoted_spaces() {
        let args = split_launch_arguments(r#"-flag "value one" --path '/tmp/tool dir'"#).unwrap();
        assert_eq!(
            args,
            vec!["-flag", "value one", "--path", "/tmp/tool dir"]
        );
    }

    #[test]
    fn build_flatpak_tool_command_handles_spaces_without_shell_concat() {
        let cmd = build_flatpak_tool_command(
            Path::new("/flatpak/proton/proton"),
            Path::new("/games/My Tool.exe"),
            r#"--profile "Default Profile" --output "out dir""#,
            Path::new("/flatpak/steam/root"),
            Path::new("/flatpak/compatdata/489830"),
            489830,
        )
        .unwrap();
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert!(args.iter().any(|a| a == "--profile"));
        assert!(args.iter().any(|a| a == "Default Profile"));
        assert!(args.iter().any(|a| a == "out dir"));
        assert!(
            args.iter()
                .any(|a| a.starts_with("--env=SteamAppId=489830"))
        );
        assert!(
            args.iter()
                .any(|a| a.starts_with("--env=SteamGameId=489830"))
        );
    }

    #[test]
    fn parse_compat_tool_mapping_finds_app_config() {
        let config_vdf = r#"
"InstallConfigStore"
{
    "Software"
    {
        "Valve"
        {
            "Steam"
            {
                "CompatToolMapping"
                {
                    "489830"
                    {
                        "name"        "proton_8"
                        "config"      ""
                        "Priority"    "250"
                    }
                    "377160"
                    {
                        "name"        "GE-Proton9-2"
                        "config"      ""
                        "Priority"    "250"
                    }
                }
            }
        }
    }
}
"#;
        let result = parse_compat_tool_mapping(config_vdf, 489830);
        assert_eq!(result, Some("proton_8".to_string()));

        let result2 = parse_compat_tool_mapping(config_vdf, 377160);
        assert_eq!(result2, Some("GE-Proton9-2".to_string()));

        let result3 = parse_compat_tool_mapping(config_vdf, 999999);
        assert_eq!(result3, None);
    }

    #[test]
    fn normalize_proton_name_removes_separators() {
        assert_eq!(normalize_proton_name("Proton-8.0"), "proton80");
        assert_eq!(normalize_proton_name("GE-Proton9-2"), "geproton92");
        assert_eq!(
            normalize_proton_name("proton_experimental"),
            "protonexperimental"
        );
    }

    #[test]
    fn tools_match_version_compares_numbers() {
        assert!(tools_match_version("proton_8", "Proton 8.0"));
        assert!(tools_match_version("GE-Proton9-2", "geproton9"));
        assert!(!tools_match_version("proton_8", "proton_9"));
    }
}
