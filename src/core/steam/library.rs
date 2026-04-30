use crate::core::games::GameKind;
use std::path::{Path, PathBuf};

pub struct SteamLibrary {
    pub path: PathBuf,
    pub apps: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedSteamGame {
    pub kind: GameKind,
    pub app_id: u32,
    pub path: PathBuf,
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
            libraries.push(SteamLibrary {
                path: steamapps_path,
                apps,
            });
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

pub(super) fn parse_vdf_key_value(line: &str) -> Option<(String, String)> {
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

pub(super) fn parse_acf_install_dir(contents: &str) -> Option<String> {
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

pub fn find_game_path(app_id: u32) -> Option<PathBuf> {
    find_game_path_in_libraries(app_id, &find_steam_libraries())
}

pub(super) fn find_game_path_in_libraries(
    app_id: u32,
    libraries: &[SteamLibrary],
) -> Option<PathBuf> {
    for lib in libraries {
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

pub fn detect_games() -> Vec<DetectedSteamGame> {
    detect_games_in_libraries(&find_steam_libraries())
}

pub(super) fn detect_games_in_libraries(libraries: &[SteamLibrary]) -> Vec<DetectedSteamGame> {
    let mut found = Vec::new();
    for kind in GameKind::all() {
        for &app_id in kind.steam_app_ids() {
            if let Some(path) = find_game_path_in_libraries(app_id, libraries) {
                found.push(DetectedSteamGame {
                    kind: kind.clone(),
                    app_id,
                    path,
                });
            }
        }
    }
    found
}

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

pub fn install_launch_options(app_id: u32, launch_options: &str) -> Result<usize, String> {
    set_launch_options(app_id, Some(launch_options))
}

pub fn clear_launch_options(app_id: u32) -> Result<usize, String> {
    set_launch_options(app_id, None)
}

pub fn read_launch_options(app_id: u32) -> Result<Option<String>, String> {
    if is_steam_flatpak() {
        return Err(
            "Flatpak Steam requires a Flatpak build of linkmm. Native linkmm cannot edit Flatpak Steam config."
                .to_string(),
        );
    }

    let steam_root =
        find_steam_root().ok_or_else(|| "Could not find Steam installation".to_string())?;
    let userdata_dir = steam_root.join("userdata");
    if !userdata_dir.is_dir() {
        return Err(format!(
            "Steam userdata directory was not found at {}",
            userdata_dir.display()
        ));
    }

    let mut seen_any = false;
    let mut values: Vec<String> = Vec::new();

    let entries = std::fs::read_dir(&userdata_dir)
        .map_err(|e| format!("Failed to read Steam userdata directory: {e}"))?;
    for entry in entries.flatten() {
        let localconfig_path = entry.path().join("config").join("localconfig.vdf");
        if !localconfig_path.is_file() {
            continue;
        }
        seen_any = true;
        let contents = std::fs::read_to_string(&localconfig_path)
            .map_err(|e| format!("Failed to read {}: {e}", localconfig_path.display()))?;
        if let Some(value) = read_launch_options_vdf(&contents, app_id)? {
            if !values.contains(&value) {
                values.push(value);
            }
        }
    }

    if !seen_any {
        return Err(format!(
            "No Steam user config files were found under {}",
            userdata_dir.display()
        ));
    }

    match values.len() {
        0 => Ok(None),
        1 => Ok(values.into_iter().next()),
        _ => Err("Steam user configs contain different launch options for this game.".to_string()),
    }
}

fn set_launch_options(app_id: u32, launch_options: Option<&str>) -> Result<usize, String> {
    if is_steam_flatpak() {
        return Err(
            "Flatpak Steam requires a Flatpak build of linkmm. Native linkmm cannot edit Flatpak Steam config."
                .to_string(),
        );
    }

    let steam_root =
        find_steam_root().ok_or_else(|| "Could not find Steam installation".to_string())?;
    let userdata_dir = steam_root.join("userdata");
    if !userdata_dir.is_dir() {
        return Err(format!(
            "Steam userdata directory was not found at {}",
            userdata_dir.display()
        ));
    }

    let mut updated = 0usize;
    let mut seen_any = false;

    let entries = std::fs::read_dir(&userdata_dir)
        .map_err(|e| format!("Failed to read Steam userdata directory: {e}"))?;
    for entry in entries.flatten() {
        let localconfig_path = entry.path().join("config").join("localconfig.vdf");
        if !localconfig_path.is_file() {
            continue;
        }
        seen_any = true;
        let contents = std::fs::read_to_string(&localconfig_path)
            .map_err(|e| format!("Failed to read {}: {e}", localconfig_path.display()))?;
        let updated_contents = set_launch_options_vdf(&contents, app_id, launch_options)?;
        if updated_contents != contents {
            std::fs::write(&localconfig_path, updated_contents)
                .map_err(|e| format!("Failed to write {}: {e}", localconfig_path.display()))?;
        }
        updated += 1;
    }

    if !seen_any {
        return Err(format!(
            "No Steam user config files were found under {}",
            userdata_dir.display()
        ));
    }

    Ok(updated)
}

pub fn is_steam_flatpak() -> bool {
    if let Some(steam_root) = find_steam_root() {
        let steam_root_str = steam_root.to_string_lossy();
        steam_root_str.contains("/.var/app/com.valvesoftware.Steam/")
    } else {
        false
    }
}

pub fn is_path_in_flatpak(path: &Path) -> bool {
    let path_str = path.to_string_lossy();
    path_str.contains("/.var/app/com.valvesoftware.Steam/")
}

pub fn find_compatdata_path(app_id: u32) -> Option<PathBuf> {
    find_compatdata_path_in_libraries(app_id, &find_steam_libraries())
}

pub(super) fn find_compatdata_path_in_libraries(
    app_id: u32,
    libraries: &[SteamLibrary],
) -> Option<PathBuf> {
    for lib in libraries {
        let manifest = lib.path.join(format!("appmanifest_{app_id}.acf"));
        if manifest.exists() {
            let path = lib.path.join("compatdata").join(app_id.to_string());
            if path.is_dir() {
                return Some(path);
            }
        }
    }
    for lib in libraries {
        let path = lib.path.join("compatdata").join(app_id.to_string());
        if path.is_dir() {
            return Some(path);
        }
    }
    None
}

fn read_launch_options_vdf(contents: &str, app_id: u32) -> Result<Option<String>, String> {
    let lines: Vec<String> = contents.lines().map(|line| line.to_string()).collect();
    let Some((_, apps_open, _apps_close)) = find_section_bounds(&lines, 0, "apps") else {
        return Err("Steam localconfig.vdf does not contain an Apps section".to_string());
    };
    let app_key = app_id.to_string();
    let Some((_, app_open, app_close)) = find_section_bounds(&lines, apps_open + 1, &app_key)
    else {
        return Ok(None);
    };
    let Some(launch_line_idx) =
        find_key_value_line(&lines, app_open + 1, app_close, "LaunchOptions")
    else {
        return Ok(None);
    };
    let (_, value) = parse_vdf_key_value(&lines[launch_line_idx])
        .ok_or_else(|| "Failed to parse LaunchOptions line in Steam config".to_string())?;
    Ok(Some(value))
}

fn set_launch_options_vdf(
    contents: &str,
    app_id: u32,
    launch_options: Option<&str>,
) -> Result<String, String> {
    let mut lines: Vec<String> = contents.lines().map(|line| line.to_string()).collect();
    let Some((_, apps_open, apps_close)) = find_section_bounds(&lines, 0, "apps") else {
        return Err("Steam localconfig.vdf does not contain an Apps section".to_string());
    };

    let app_key = app_id.to_string();
    if let Some((_, app_open, app_close)) = find_section_bounds(&lines, apps_open + 1, &app_key) {
        if let Some(launch_line_idx) =
            find_key_value_line(&lines, app_open + 1, app_close, "LaunchOptions")
        {
            if let Some(launch_options) = launch_options {
                let indent = leading_whitespace(&lines[launch_line_idx]);
                lines[launch_line_idx] = format!(
                    "{indent}\"LaunchOptions\"\t\t\"{}\"",
                    escape_vdf_string(launch_options)
                );
            } else {
                lines.remove(launch_line_idx);
            }
        } else if let Some(launch_options) = launch_options {
            let indent = format!("{}\t", leading_whitespace(&lines[app_close]));
            lines.insert(
                app_close,
                format!(
                    "{indent}\"LaunchOptions\"\t\t\"{}\"",
                    escape_vdf_string(launch_options)
                ),
            );
        }
    } else {
        if let Some(launch_options) = launch_options {
            let app_indent = format!("{}\t", leading_whitespace(&lines[apps_close]));
            let value_indent = format!("{app_indent}\t");
            let insert_at = apps_close;
            lines.insert(insert_at, format!("{app_indent}\"{app_key}\""));
            lines.insert(insert_at + 1, format!("{app_indent}{{"));
            lines.insert(
                insert_at + 2,
                format!(
                    "{value_indent}\"LaunchOptions\"\t\t\"{}\"",
                    escape_vdf_string(launch_options)
                ),
            );
            lines.insert(insert_at + 3, format!("{app_indent}}}"));
        }
    }

    let mut output = lines.join("\n");
    if contents.ends_with('\n') {
        output.push('\n');
    }
    Ok(output)
}

fn find_section_bounds(
    lines: &[String],
    start_idx: usize,
    section_name: &str,
) -> Option<(usize, usize, usize)> {
    let target = section_name.to_ascii_lowercase();
    let mut idx = start_idx;
    while idx < lines.len() {
        if let Some(name) = parse_vdf_section_name(&lines[idx])
            && name.eq_ignore_ascii_case(&target)
        {
            let mut open_idx = idx + 1;
            while open_idx < lines.len() && lines[open_idx].trim().is_empty() {
                open_idx += 1;
            }
            if open_idx < lines.len() && lines[open_idx].trim() == "{" {
                let close_idx = find_matching_brace(lines, open_idx)?;
                return Some((idx, open_idx, close_idx));
            }
        }
        idx += 1;
    }
    None
}

fn find_matching_brace(lines: &[String], open_idx: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (idx, line) in lines.iter().enumerate().skip(open_idx) {
        match line.trim() {
            "{" => depth += 1,
            "}" => {
                depth -= 1;
                if depth == 0 {
                    return Some(idx);
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_vdf_section_name(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if !trimmed.starts_with('"') {
        return None;
    }
    let rest = &trimmed[1..];
    let key_end = rest.find('"')?;
    let after_key = rest[key_end + 1..].trim();
    if !after_key.is_empty() {
        return None;
    }
    Some(rest[..key_end].to_string())
}

fn find_key_value_line(
    lines: &[String],
    start_idx: usize,
    end_idx: usize,
    key_name: &str,
) -> Option<usize> {
    for (idx, line) in lines.iter().enumerate().take(end_idx).skip(start_idx) {
        if let Some((key, _)) = parse_vdf_key_value(line)
            && key.eq_ignore_ascii_case(key_name)
        {
            return Some(idx);
        }
    }
    None
}

fn leading_whitespace(line: &str) -> String {
    line.chars().take_while(|c| c.is_whitespace()).collect()
}

fn escape_vdf_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::games::GameKind;
    use tempfile::TempDir;

    #[test]
    fn is_steam_flatpak_detects_flatpak_path() {
        let _ = is_steam_flatpak();
    }

    #[test]
    fn is_path_in_flatpak_detects_flatpak_paths() {
        let flatpak_path = PathBuf::from(
            "/home/user/.var/app/com.valvesoftware.Steam/.steam/steam/compatibilitytools.d/GE-Proton10-34",
        );
        assert!(is_path_in_flatpak(&flatpak_path));

        let flatpak_compatdata = PathBuf::from(
            "/home/user/.var/app/com.valvesoftware.Steam/data/steamapps/compatdata/489830",
        );
        assert!(is_path_in_flatpak(&flatpak_compatdata));

        let native_path =
            PathBuf::from("/home/user/.local/share/Steam/steamapps/common/Proton 8.0");
        assert!(!is_path_in_flatpak(&native_path));

        let external_lib = PathBuf::from("/mnt/data0/.steamlib/steamapps/compatdata/489830");
        assert!(!is_path_in_flatpak(&external_lib));
    }

    #[test]
    fn detect_games_recognizes_fallout_nv_alias_without_duplicates() {
        let tmp = TempDir::new().expect("tempdir");
        let steamapps = tmp.path().join("steamapps");
        let common = steamapps.join("common");
        let game_dir = common.join("Fallout New Vegas PCR");
        std::fs::create_dir_all(&game_dir).expect("create game dir");

        let manifest = steamapps.join("appmanifest_22490.acf");
        let manifest_body = r#"
            "AppState"
            {
                "appid"      "22490"
                "installdir" "Fallout New Vegas PCR"
            }
        "#;
        std::fs::create_dir_all(&steamapps).expect("create steamapps");
        std::fs::write(&manifest, manifest_body).expect("write manifest");

        let libraries = vec![SteamLibrary {
            path: steamapps,
            apps: vec![22490],
        }];

        let detected = detect_games_in_libraries(&libraries);
        let fallout_nv: Vec<_> = detected
            .iter()
            .filter(|entry| entry.kind == GameKind::FalloutNV)
            .collect();
        assert_eq!(fallout_nv.len(), 1);
        assert_eq!(fallout_nv[0].app_id, 22490);
        assert_eq!(fallout_nv[0].path, game_dir);
    }

    #[test]
    fn detect_games_keeps_both_fallout_nv_steam_instances() {
        let tmp = TempDir::new().expect("tempdir");
        let steamapps = tmp.path().join("steamapps");
        std::fs::create_dir_all(steamapps.join("common").join("Fallout New Vegas"))
            .expect("create fnv dir");
        std::fs::create_dir_all(steamapps.join("common").join("Fallout New Vegas PCR"))
            .expect("create pcr dir");

        std::fs::write(
            steamapps.join("appmanifest_22380.acf"),
            "\"AppState\"\n{\n\t\"appid\"\t\"22380\"\n\t\"installdir\"\t\"Fallout New Vegas\"\n}\n",
        )
        .expect("write fnv manifest");
        std::fs::write(
            steamapps.join("appmanifest_22490.acf"),
            "\"AppState\"\n{\n\t\"appid\"\t\"22490\"\n\t\"installdir\"\t\"Fallout New Vegas PCR\"\n}\n",
        )
        .expect("write pcr manifest");

        let libraries = vec![SteamLibrary {
            path: steamapps.clone(),
            apps: vec![22380, 22490],
        }];

        let detected = detect_games_in_libraries(&libraries);
        let fallout_nv: Vec<_> = detected
            .into_iter()
            .filter(|entry| entry.kind == GameKind::FalloutNV)
            .collect();
        assert_eq!(fallout_nv.len(), 2);
        assert!(fallout_nv.iter().any(|entry| entry.app_id == 22380));
        assert!(fallout_nv.iter().any(|entry| entry.app_id == 22490));
    }

    #[test]
    fn compatdata_lookup_uses_instance_app_id() {
        let tmp = TempDir::new().expect("tempdir");
        let steamapps = tmp.path().join("steamapps");
        let compat_22380 = steamapps.join("compatdata").join("22380");
        let compat_22490 = steamapps.join("compatdata").join("22490");
        std::fs::create_dir_all(&compat_22380).expect("create 22380 compatdata");
        std::fs::create_dir_all(&compat_22490).expect("create 22490 compatdata");
        std::fs::write(steamapps.join("appmanifest_22490.acf"), "\"AppState\"{}")
            .expect("write 22490 manifest");

        let libraries = vec![SteamLibrary {
            path: steamapps,
            apps: vec![22380, 22490],
        }];

        assert_eq!(
            find_compatdata_path_in_libraries(22490, &libraries),
            Some(compat_22490)
        );
        assert_eq!(
            find_compatdata_path_in_libraries(22380, &libraries),
            Some(compat_22380)
        );
    }

    #[test]
    fn upsert_launch_options_replaces_existing_value() {
        let input = r#""UserLocalConfigStore"
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
                        "LaunchOptions"        "old value"
                    }
                }
            }
        }
    }
}
"#;
        let output = set_launch_options_vdf(input, 489830, Some("new value")).unwrap();
        assert!(output.contains("\"LaunchOptions\"\t\t\"new value\""));
        assert!(!output.contains("old value"));
    }

    #[test]
    fn upsert_launch_options_inserts_app_block_when_missing() {
        let input = r#""UserLocalConfigStore"
{
    "Software"
    {
        "Valve"
        {
            "Steam"
            {
                "apps"
                {
                }
            }
        }
    }
}
"#;
        let output = set_launch_options_vdf(input, 489830, Some("launch")).unwrap();
        assert!(output.contains("\"489830\""));
        assert!(output.contains("\"LaunchOptions\"\t\t\"launch\""));
    }

    #[test]
    fn set_launch_options_removes_existing_value() {
        let input = r#""UserLocalConfigStore"
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
                        "LaunchOptions"        "old value"
                    }
                }
            }
        }
    }
}
"#;
        let output = set_launch_options_vdf(input, 489830, None).unwrap();
        assert!(!output.contains("LaunchOptions"));
    }

    #[test]
    fn read_launch_options_extracts_existing_value() {
        let input = r#""UserLocalConfigStore"
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
                        "LaunchOptions"        "old value"
                    }
                }
            }
        }
    }
}
"#;
        let value = read_launch_options_vdf(input, 489830).unwrap();
        assert_eq!(value.as_deref(), Some("old value"));
    }
}
