use std::path::PathBuf;

use super::library::{find_compatdata_path, find_steam_libraries, parse_vdf_key_value};

/// Parse config.vdf to find the per-game compatibility tool name for `app_id`.
pub(super) fn parse_per_game_proton_config(app_id: u32) -> Option<String> {
    let home = dirs::home_dir()?;

    let candidate_config_paths = vec![
        home.join(".steam").join("steam").join("config").join("config.vdf"),
        home.join(".local").join("share").join("Steam").join("config").join("config.vdf"),
        home.join("snap").join("steam").join("common").join(".steam").join("steam").join("config").join("config.vdf"),
        home.join(".var").join("app").join("com.valvesoftware.Steam").join(".steam").join("steam").join("config").join("config.vdf"),
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

/// Parse the CompatToolMapping section from config.vdf for `app_id`.
pub(super) fn parse_compat_tool_mapping(contents: &str, app_id: u32) -> Option<String> {
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
                continue;
            }
            if trimmed.starts_with(&format!("\"{}\"", app_id_str)) {
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
        if let Some(steamapps_parent) = lib.path.parent() {
            let common_path = steamapps_parent.join("common");
            if common_path.is_dir() {
                proton_dirs.push(common_path);
            }
        }
        if let Some(steam_root) = lib.path.parent() {
            let compat_tools_path = steam_root.join("compatibilitytools.d");
            if compat_tools_path.is_dir() {
                proton_dirs.push(compat_tools_path);
            }
        }
    }

    if let Some(home) = dirs::home_dir() {
        let roots = vec![
            home.join(".steam").join("steam"),
            home.join(".local").join("share").join("Steam"),
            home.join(".var").join("app").join("com.valvesoftware.Steam").join(".steam").join("steam"),
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

/// Search for a Proton installation by name across steamapps/common and compatibilitytools.d.
fn find_proton_by_name(tool_name: &str) -> Option<PathBuf> {
    for base_dir in &find_proton_directories() {
        if let Ok(entries) = std::fs::read_dir(base_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let dir_name = entry.file_name().to_string_lossy().to_string();

                if (dir_name == tool_name
                    || normalize_proton_name(&dir_name) == normalize_proton_name(tool_name))
                    && path.join("proton").exists()
                {
                    log::debug!("Found Proton at: {}", path.display());
                    return Some(path);
                }

                if tool_name.contains("proton") && dir_name.to_lowercase().contains("proton") {
                    let tool_lower = tool_name.to_lowercase();
                    let dir_lower = dir_name.to_lowercase();
                    if tools_match_version(&tool_lower, &dir_lower) && path.join("proton").exists() {
                        log::debug!("Found Proton at: {} (version match)", path.display());
                        return Some(path);
                    }
                }
            }
        }
    }
    None
}

fn normalize_proton_name(name: &str) -> String {
    name.to_lowercase()
        .replace("-", "")
        .replace("_", "")
        .replace(" ", "")
        .replace(".", "")
}

fn tools_match_version(tool1: &str, tool2: &str) -> bool {
    let nums1: Vec<&str> = tool1
        .split(|c: char| !c.is_numeric())
        .filter(|s| !s.is_empty())
        .collect();
    let nums2: Vec<&str> = tool2
        .split(|c: char| !c.is_numeric())
        .filter(|s| !s.is_empty())
        .collect();
    if !nums1.is_empty() && !nums2.is_empty() {
        nums1.first() == nums2.first()
    } else {
        false
    }
}

/// Find the Proton runtime path and compatdata directory for a game's App ID.
///
/// Resolution order:
/// 1. Per-game config.vdf CompatToolMapping
/// 2. compatdata/version file
/// 3. Automatic scan (newest Proton found)
pub fn find_proton_for_game(app_id: u32) -> Result<(PathBuf, PathBuf), String> {
    let compatdata_path = find_compatdata_path(app_id)
        .ok_or_else(|| format!("Could not find compatdata for App ID {}", app_id))?;

    log::debug!("Finding Proton for app_id: {}", app_id);

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

    let tool_manifest = compatdata_path.join("version");
    let proton_version = if tool_manifest.exists() {
        std::fs::read_to_string(&tool_manifest)
            .ok()
            .and_then(|content| content.lines().next().map(|s| s.trim().to_string()))
    } else {
        None
    };

    if let Some(ref version) = proton_version {
        log::debug!("Compatdata version file contains: {}", version);
        if let Some(proton_path) = find_proton_by_name(version) {
            log::info!("Found Proton matching version file: {}", proton_path.display());
            return Ok((proton_path, compatdata_path));
        }
    }

    log::debug!("Falling back to automatic Proton detection");
    for base_dir in &find_proton_directories() {
        if let Ok(entries) = std::fs::read_dir(base_dir) {
            let mut candidates: Vec<_> = entries
                .flatten()
                .filter(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    name.to_lowercase().contains("proton") && e.path().join("proton").exists()
                })
                .collect();

            candidates.sort_by(|a, b| {
                b.file_name()
                    .to_string_lossy()
                    .cmp(&a.file_name().to_string_lossy())
            });

            if let Some(proton_dir) = candidates.first() {
                let proton_path = proton_dir.path();
                log::info!("Using fallback Proton: {}", proton_path.display());
                return Ok((proton_path, compatdata_path));
            }
        }
    }

    Err("Could not find any Proton installation".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(normalize_proton_name("proton_experimental"), "protonexperimental");
    }

    #[test]
    fn tools_match_version_compares_numbers() {
        assert!(tools_match_version("proton_8", "Proton 8.0"));
        assert!(tools_match_version("GE-Proton9-2", "geproton9"));
        assert!(!tools_match_version("proton_8", "proton_9"));
    }
}
