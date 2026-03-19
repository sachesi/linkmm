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

// ── localconfig.vdf helpers ───────────────────────────────────────────────

/// Parse a VDF line that is a bare section-header key with no associated
/// value, e.g. `"apps"`.  Returns `None` for lines that carry a value
/// (handled by [`parse_vdf_key_value`]) or are not quoted strings.
fn parse_vdf_section_name(line: &str) -> Option<String> {
    let line = line.trim();
    if !line.starts_with('"') {
        return None;
    }
    let rest = &line[1..];
    let key_end = rest.find('"')?;
    let key = rest[..key_end].to_string();
    // A bare section key has nothing after the closing quote.
    if rest[key_end + 1..].trim().is_empty() {
        Some(key)
    } else {
        None
    }
}

/// Return the leading whitespace prefix of `line` (tabs and spaces).
fn leading_whitespace(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

/// Escape a string for use as a value inside VDF double-quotes.
fn escape_vdf_value(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Format a string as a Python string literal (double-quoted, backslash-escaped).
fn python_str_literal(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Return the path to Steam's `localconfig.vdf` for the active user account.
///
/// The file is at `<steam_root>/userdata/<userid>/config/localconfig.vdf`.
/// When multiple user directories are present the most recently modified file
/// is returned.  Returns `None` when Steam is not found or no user has a
/// `localconfig.vdf`.
pub fn find_localconfig_vdf() -> Option<PathBuf> {
    let steam_root = find_steam_root()?;
    let userdata = steam_root.join("userdata");
    if !userdata.is_dir() {
        return None;
    }

    let mut best: Option<(PathBuf, std::time::SystemTime)> = None;
    if let Ok(entries) = std::fs::read_dir(&userdata) {
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let cfg = entry.path().join("config").join("localconfig.vdf");
            if cfg.is_file() {
                let mtime = cfg
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                if best.as_ref().map(|(_, t)| mtime > *t).unwrap_or(true) {
                    best = Some((cfg, mtime));
                }
            }
        }
    }
    best.map(|(p, _)| p)
}

/// Edit `localconfig.vdf` text so that the `LaunchOptions` key for `app_id`
/// is set to `opts`.
///
/// Three cases are handled:
/// * `LaunchOptions` already exists inside the app section → value replaced.
/// * The app section exists but has no `LaunchOptions` key → key inserted.
/// * The app section does not exist under `apps` → section + key inserted.
///
/// If the top-level `apps` section cannot be found the content is returned
/// unmodified (graceful degradation for unusual Steam installations).
pub fn set_launch_options_in_vdf(contents: &str, app_id: u32, opts: &str) -> String {
    let app_id_str = app_id.to_string();
    let escaped_opts = escape_vdf_value(opts);

    #[derive(PartialEq)]
    enum Phase {
        SeekingApps,
        InApps,
        InApp,
        Done,
    }

    let mut out = String::with_capacity(contents.len() + 256);
    let mut phase = Phase::SeekingApps;
    let mut depth: i32 = 0;
    let mut apps_depth: i32 = -1;
    let mut app_depth: i32 = -1;
    let mut pending_key: Option<String> = None;
    let mut launch_options_written = false;

    for line in contents.lines() {
        let trimmed = line.trim();

        if trimmed == "{" {
            depth += 1;
            match phase {
                Phase::SeekingApps => {
                    if pending_key
                        .as_deref()
                        .map(|k| k.eq_ignore_ascii_case("apps"))
                        .unwrap_or(false)
                    {
                        phase = Phase::InApps;
                        apps_depth = depth;
                    }
                }
                Phase::InApps => {
                    if pending_key.as_deref() == Some(app_id_str.as_str()) {
                        phase = Phase::InApp;
                        app_depth = depth;
                    }
                }
                _ => {}
            }
            pending_key = None;
            out.push_str(line);
            out.push('\n');
            continue;
        }

        if trimmed == "}" {
            match phase {
                Phase::InApp if depth == app_depth => {
                    // Leaving the app section — insert LaunchOptions if it was
                    // never encountered in this section.
                    if !launch_options_written {
                        let ind = leading_whitespace(line);
                        out.push_str(ind);
                        out.push('\t');
                        out.push_str(&format!(
                            "\"LaunchOptions\"\t\t\"{escaped_opts}\"\n"
                        ));
                        launch_options_written = true;
                    }
                    phase = Phase::Done;
                    app_depth = -1;
                }
                Phase::InApps if depth == apps_depth => {
                    // Leaving the apps section without finding the app — insert
                    // the whole section before the closing brace.
                    let ind = leading_whitespace(line);
                    out.push_str(ind);
                    out.push('\t');
                    out.push_str(&format!("\"{app_id_str}\"\n"));
                    out.push_str(ind);
                    out.push('\t');
                    out.push_str("{\n");
                    out.push_str(ind);
                    out.push_str("\t\t");
                    out.push_str(&format!("\"LaunchOptions\"\t\t\"{escaped_opts}\"\n"));
                    out.push_str(ind);
                    out.push('\t');
                    out.push_str("}\n");
                    launch_options_written = true;
                    phase = Phase::Done;
                    apps_depth = -1;
                }
                _ => {}
            }
            depth -= 1;
            pending_key = None;
            out.push_str(line);
            out.push('\n');
            continue;
        }

        // Key-value pair or bare section-name key.
        if let Some((key, _)) = parse_vdf_key_value(trimmed) {
            if phase == Phase::InApp && key.eq_ignore_ascii_case("launchoptions") {
                // Replace the value in-place, preserving the line's indentation.
                let ind = leading_whitespace(line);
                out.push_str(ind);
                out.push_str(&format!("\"LaunchOptions\"\t\t\"{escaped_opts}\"\n"));
                launch_options_written = true;
                pending_key = None;
                continue; // skip pushing the original line
            }
            pending_key = None;
        } else if let Some(name) = parse_vdf_section_name(trimmed) {
            pending_key = Some(name);
        } else {
            pending_key = None;
        }

        out.push_str(line);
        out.push('\n');
    }

    out
}

/// Write a Python 3 wrapper script to `/tmp/linkmm_<app_id>_launch.py`.
///
/// The script is invoked by Steam as `<script> %command%`, where `%command%`
/// expands to Steam's full Proton launch command.  The script replaces the
/// argument that ends with `default_exe` with the full path to `chosen_exe`,
/// then `exec`s into the modified command.  This causes Proton to run the
/// chosen executable (e.g. `skse64_loader.exe`) while Steam still tracks the
/// game session through its normal process-monitoring infrastructure.
fn write_exe_override_script(
    app_id: u32,
    default_exe: &str,
    chosen_exe: &PathBuf,
) -> Result<PathBuf, String> {
    let script_path = PathBuf::from(format!("/tmp/linkmm_{app_id}_launch.py"));

    let default_py = python_str_literal(default_exe);
    let chosen_py = python_str_literal(&chosen_exe.to_string_lossy());

    // The script substitutes `default_exe` for `chosen_exe` in argv, matching
    // by the final path component so it works regardless of the full path that
    // Steam embeds in the Proton command.
    let script = format!(
        "#!/usr/bin/env python3\n\
         # Generated by linkmm — do not edit\n\
         import sys, os\n\
         DEFAULT = {default_py}\n\
         CHOSEN  = {chosen_py}\n\
         args = list(sys.argv[1:])\n\
         for i, a in enumerate(args):\n\
         \tif a == DEFAULT or a.endswith(\"/\" + DEFAULT) or a.endswith(\"\\\\\\\\\" + DEFAULT):\n\
         \t\targs[i] = CHOSEN\n\
         \t\tbreak\n\
         if args:\n\
         \tos.execvp(args[0], args)\n"
    );

    std::fs::write(&script_path, script.as_bytes())
        .map_err(|e| format!("Failed to write launcher script: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("Failed to make launcher script executable: {e}"))?;
    }

    Ok(script_path)
}

/// Atomically update the `LaunchOptions` for `app_id` in Steam's
/// `localconfig.vdf`.
///
/// Pass an empty string to clear any previously set launch override (Steam
/// will use its default executable).
///
/// Returns an error when `localconfig.vdf` cannot be found or written.
pub fn set_steam_launch_option(app_id: u32, opts: &str) -> Result<(), String> {
    let path = find_localconfig_vdf()
        .ok_or_else(|| "Steam localconfig.vdf not found".to_string())?;

    let contents = std::fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;

    let modified = set_launch_options_in_vdf(&contents, app_id, opts);

    // Write to a temp file first, then rename for atomicity.
    let tmp = path.with_extension("vdf.linkmm_tmp");
    std::fs::write(&tmp, modified.as_bytes())
        .map_err(|e| format!("Failed to write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("Failed to update {}: {e}", path.display())
    })?;

    Ok(())
}

/// Launch a game executable, always routing through the Steam client.
///
/// For the **primary** game binary any previously written exe override is
/// cleared (Steam's default launch is restored) before opening
/// `steam://run/<app_id>`.
///
/// For **non-primary** executables (script-extender loaders such as SKSE64,
/// F4SE, NVSE, OBSE, …) a small Python 3 wrapper script is written to `/tmp`
/// and registered as the game's `LaunchOptions` in Steam's
/// `userdata/.../localconfig.vdf`.  The wrapper intercepts Steam's Proton
/// invocation and substitutes the chosen executable for the default one,
/// so that Proton runs the script extender while Steam still tracks the
/// session: the game appears as "Running" in the library, the overlay is
/// active, and playtime is counted.
///
/// All launches use `xdg-open steam://run/<app_id>` so the Steam client is
/// always the process that starts the game.
///
/// Returns `Ok(())` on successful spawn, or an error message string on
/// failure.
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

    let is_primary = game
        .kind
        .known_executables()
        .first()
        .map(|&primary| primary.eq_ignore_ascii_case(exe_name))
        .unwrap_or(false);

    if is_primary {
        // Clear any exe override left by a previous script-extender launch.
        // Ignore errors — the Steam launch still works without this step.
        let _ = set_steam_launch_option(app_id, "");
    } else {
        // Write a launcher wrapper that substitutes the chosen exe inside
        // Steam's Proton command, then register it as the game's LaunchOptions
        // so that `steam://run/<app_id>` runs the chosen binary through Steam.
        let default_exe = game
            .kind
            .known_executables()
            .first()
            .copied()
            .unwrap_or("");
        let script = write_exe_override_script(app_id, default_exe, &exe_path)?;
        set_steam_launch_option(
            app_id,
            &format!("\"{}\" %command%", script.display()),
        )?;
    }

    // Always launch via Steam so playtime, overlay, and "Running" status work.
    std::process::Command::new("xdg-open")
        .arg(format!("steam://run/{app_id}"))
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Failed to open steam://run/{app_id}: {e}"))
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
        // The executable-existence check happens before any Steam/localconfig
        // lookup, so a missing loader exe returns "Executable not found" immediately.
        let tmp = tempfile::tempdir().expect("tempdir");
        let game = Game::new(GameKind::SkyrimSE, tmp.path().to_path_buf());
        let result = launch_game_executable(&game, "skse64_loader.exe");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Executable not found"));
    }

    #[test]
    fn launch_game_executable_script_extender_fails_without_localconfig() {
        // When the exe exists on disk but Steam is not installed (no
        // localconfig.vdf), launching a non-primary exe should fail with a
        // descriptive error about localconfig.vdf.
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("skse64_loader.exe"), b"fake").unwrap();
        let game = Game::new(GameKind::SkyrimSE, tmp.path().to_path_buf());
        let result = launch_game_executable(&game, "skse64_loader.exe");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("localconfig") || err.contains("launcher script"),
            "expected error about localconfig.vdf or launcher script, got: {err}"
        );
    }

    // ── set_launch_options_in_vdf tests ──────────────────────────────────────

    /// Minimal localconfig.vdf with a LaunchOptions key already present.
    const VDF_WITH_LAUNCH_OPTIONS: &str = r#""UserLocalConfigStore"
{
	"Software"
	{
		"Valve"
		{
			"Steam"
			{
				"apps"
				{
					"489830"
					{
						"LaunchOptions"		""
						"LastPlayed"		"1000"
					}
				}
			}
		}
	}
}
"#;

    /// Minimal localconfig.vdf where the app section exists but has no
    /// LaunchOptions key.
    const VDF_WITHOUT_LAUNCH_OPTIONS_KEY: &str = r#""UserLocalConfigStore"
{
	"Software"
	{
		"Valve"
		{
			"Steam"
			{
				"apps"
				{
					"489830"
					{
						"LastPlayed"		"1000"
					}
				}
			}
		}
	}
}
"#;

    /// Minimal localconfig.vdf where the target app section is absent.
    const VDF_WITHOUT_APP_SECTION: &str = r#""UserLocalConfigStore"
{
	"Software"
	{
		"Valve"
		{
			"Steam"
			{
				"apps"
				{
					"99999"
					{
						"LastPlayed"		"1000"
					}
				}
			}
		}
	}
}
"#;

    #[test]
    fn set_launch_options_replaces_existing_key() {
        let result = set_launch_options_in_vdf(VDF_WITH_LAUNCH_OPTIONS, 489830, "/tmp/x.py %command%");
        assert!(result.contains("\"LaunchOptions\""));
        assert!(result.contains("/tmp/x.py %command%"), "new value should appear");
        // The old empty value should be gone
        assert!(
            !result.contains("\"LaunchOptions\"\t\t\"\""),
            "old empty value should be replaced"
        );
        // Unrelated key must be preserved
        assert!(result.contains("\"LastPlayed\""));
    }

    #[test]
    fn set_launch_options_inserts_key_when_missing() {
        let result = set_launch_options_in_vdf(
            VDF_WITHOUT_LAUNCH_OPTIONS_KEY,
            489830,
            "/tmp/x.py %command%",
        );
        assert!(result.contains("\"LaunchOptions\""), "key should be inserted");
        assert!(result.contains("/tmp/x.py %command%"));
        assert!(result.contains("\"LastPlayed\""), "existing key preserved");
    }

    #[test]
    fn set_launch_options_inserts_app_section_when_missing() {
        let result = set_launch_options_in_vdf(VDF_WITHOUT_APP_SECTION, 489830, "myopts");
        assert!(result.contains("\"489830\""), "app section should be created");
        assert!(result.contains("\"LaunchOptions\""));
        assert!(result.contains("myopts"));
        assert!(result.contains("\"99999\""), "other app section preserved");
    }

    #[test]
    fn set_launch_options_clears_when_opts_empty() {
        let vdf = VDF_WITH_LAUNCH_OPTIONS.replace("\"\"", "\"/old/script.py %command%\"");
        let result = set_launch_options_in_vdf(&vdf, 489830, "");
        assert!(result.contains("\"LaunchOptions\""));
        assert!(
            !result.contains("/old/script.py"),
            "old launch options should be removed"
        );
    }

    #[test]
    fn set_launch_options_vdf_value_is_escaped() {
        // Backslashes and double-quotes in opts must be escaped in the output.
        let result = set_launch_options_in_vdf(VDF_WITH_LAUNCH_OPTIONS, 489830, r#"a\b"c"#);
        assert!(result.contains(r#"a\\b\"c"#), "special chars should be escaped");
    }
}
