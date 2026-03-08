use std::io::{BufWriter, Read};
use std::path::Path;
use std::process::Command;

use crate::core::games::Game;
use crate::core::mods::{Mod, ModDatabase, ModManager};

// ── Install strategy ──────────────────────────────────────────────────────────

/// How a mod archive should be installed.
#[derive(Debug, Clone)]
pub enum InstallStrategy {
    /// Extract to a mod folder under a `Data/` subdirectory and symlink into
    /// `<game_root>/Data`.
    Data,
    /// FOMOD-guided installation.  The `Vec<FomodFile>` contains the resolved
    /// list of files to install based on user selections.
    Fomod(Vec<FomodFile>),
}

// ── FOMOD types ───────────────────────────────────────────────────────────────
const FOMOD_DIR_PREFIX: &str = "fomod/";

/// A single file/folder mapping inside a FOMOD config.
#[derive(Debug, Clone)]
pub struct FomodFile {
    pub source: String,
    pub destination: String,
    pub priority: i32,
}

/// Selection type for a FOMOD plugin group.
#[derive(Debug, Clone, PartialEq)]
pub enum GroupType {
    SelectAtLeastOne,
    SelectAtMostOne,
    SelectExactlyOne,
    SelectAll,
    SelectAny,
}

/// A single selectable plugin inside a FOMOD group.
#[derive(Debug, Clone)]
pub struct FomodPlugin {
    pub name: String,
    pub description: Option<String>,
    pub image_path: Option<String>,
    pub files: Vec<FomodFile>,
    pub type_descriptor: PluginType,
    pub condition_flags: Vec<ConditionFlag>,
    pub dependencies: Option<PluginDependencies>,
}

/// Condition flag contributed by a selected plugin.
#[derive(Debug, Clone, PartialEq)]
pub struct ConditionFlag {
    pub name: String,
    pub value: String,
}

/// A single flag dependency for plugin visibility.
#[derive(Debug, Clone, PartialEq)]
pub struct FlagDependency {
    pub flag: String,
    pub value: String,
}

/// Logical operator used for plugin dependency checks.
#[derive(Debug, Clone, PartialEq)]
pub enum DependencyOperator {
    And,
    Or,
}

/// Plugin visibility dependencies declared in FOMOD.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginDependencies {
    pub operator: DependencyOperator,
    pub flags: Vec<FlagDependency>,
}

/// Default selection state of a FOMOD plugin.
#[derive(Debug, Clone, PartialEq)]
pub enum PluginType {
    Required,
    Optional,
    Recommended,
    NotUsable,
}

/// A group of plugins that the user must choose from.
#[derive(Debug, Clone)]
pub struct PluginGroup {
    pub name: String,
    pub group_type: GroupType,
    pub plugins: Vec<FomodPlugin>,
}

/// A single install step presented in the FOMOD wizard.
#[derive(Debug, Clone)]
pub struct InstallStep {
    pub name: String,
    pub visible: Option<PluginDependencies>,
    pub groups: Vec<PluginGroup>,
}

/// Conditional files selected by dependency flags under
/// `<conditionalFileInstalls>`.
#[derive(Debug, Clone)]
pub struct ConditionalFileInstall {
    pub dependencies: PluginDependencies,
    pub files: Vec<FomodFile>,
}

/// Parsed FOMOD configuration.
#[derive(Debug, Clone)]
pub struct FomodConfig {
    pub mod_name: Option<String>,
    pub required_files: Vec<FomodFile>,
    pub steps: Vec<InstallStep>,
    pub conditional_file_installs: Vec<ConditionalFileInstall>,
}

// ── Strategy detection ────────────────────────────────────────────────────────

/// Examine the contents of a zip archive and determine the best install
/// strategy.
///
/// - If a `fomod/ModuleConfig.xml` is found → `Fomod` (with empty file list;
///   the caller must run the wizard to populate it).
/// - Otherwise → `Data` (all content is placed under a `Data/` subdirectory
///   inside the mod folder and symlinked into `<game_root>/Data`).
pub fn detect_strategy(archive_path: &Path) -> Result<InstallStrategy, String> {
    if !archive_path.exists() {
        return Err(format!("Cannot open archive: {}", archive_path.display()));
    }
    if !has_zip_extension(archive_path) {
        return match parse_fomod_from_archive(archive_path) {
            Ok(config) => Ok(InstallStrategy::Fomod(config.required_files.clone())),
            Err(_) => Ok(InstallStrategy::Data),
        };
    }

    let file =
        std::fs::File::open(archive_path).map_err(|e| format!("Cannot open archive: {e}"))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| format!("Cannot read zip archive: {e}"))?;

    let mut has_fomod = false;

    for i in 0..zip.len() {
        let entry = zip
            .by_index(i)
            .map_err(|e| format!("Cannot read zip entry: {e}"))?;
        let name_lower = entry.name().to_lowercase();

        if name_lower == "fomod/moduleconfig.xml" || name_lower.ends_with("/fomod/moduleconfig.xml")
        {
            has_fomod = true;
        }
    }

    if has_fomod {
        let config = parse_fomod_from_zip(archive_path)?;
        // Return Fomod with empty file list – the caller will run the wizard
        Ok(InstallStrategy::Fomod(config.required_files.clone()))
    } else {
        Ok(InstallStrategy::Data)
    }
}

fn top_level_component(path: &str) -> &str {
    let stripped = path.strip_prefix('/').unwrap_or(path);
    stripped.split('/').next().unwrap_or("")
}

/// Return `true` when the archive already contains a `Data/` folder after the
/// common wrapper prefix is stripped.
///
/// Examples:
/// - `Data/textures/sky.dds` → common prefix `Data/` stripped → bare files →
///   returns `false` (content needs to go into `Data/`).
/// - `SomeMod/Data/textures/sky.dds` → common prefix `SomeMod/` stripped →
///   remaining starts with `Data/` → returns `true`.
/// - `textures/sky.dds` → no prefix → returns `false`.
fn archive_has_data_folder(archive_path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(archive_path) else {
        return false;
    };
    let Ok(mut zip) = zip::ZipArchive::new(file) else {
        return false;
    };
    // find_common_prefix now uses file_names() (&self), so we can reuse the
    // same ZipArchive instance for the entry scan below.
    let prefix = find_common_prefix(&zip);

    for i in 0..zip.len() {
        let Ok(entry) = zip.by_index(i) else {
            continue;
        };
        let name = entry.name();
        let rel = if !prefix.is_empty() {
            name.strip_prefix(&prefix).unwrap_or(name)
        } else {
            name
        };
        let rel_lower = rel.to_lowercase();
        let top = top_level_component(&rel_lower);
        if top == "data" {
            return true;
        }
    }
    false
}

// ── FOMOD XML parser ──────────────────────────────────────────────────────────

/// Parse a FOMOD `ModuleConfig.xml` from a supported archive.
pub fn parse_fomod_from_archive(archive_path: &Path) -> Result<FomodConfig, String> {
    if has_zip_extension(archive_path) {
        return parse_fomod_from_zip(archive_path);
    }

    let tmp_extract = create_temp_extract_dir()?;
    extract_archive_with_7z(archive_path, &tmp_extract)?;

    let result = (|| {
        let config_path = find_fomod_config_in_dir(&tmp_extract)
            .ok_or_else(|| "No fomod/ModuleConfig.xml found in archive".to_string())?;
        let xml_bytes = std::fs::read(&config_path)
            .map_err(|e| format!("Failed to read fomod config {}: {e}", config_path.display()))?;
        parse_fomod_xml(&xml_bytes)
    })();

    if let Err(e) = std::fs::remove_dir_all(&tmp_extract) {
        log::warn!(
            "Failed to remove temporary extraction directory {}: {e}",
            tmp_extract.display()
        );
    }

    result
}

/// Parse a FOMOD `ModuleConfig.xml` from inside a zip archive.
pub fn parse_fomod_from_zip(archive_path: &Path) -> Result<FomodConfig, String> {
    let file =
        std::fs::File::open(archive_path).map_err(|e| format!("Cannot open archive: {e}"))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| format!("Cannot read zip archive: {e}"))?;

    // Find the fomod config file (case-insensitive)
    let fomod_entry_name = find_fomod_entry(&mut zip)?;

    let mut entry = zip
        .by_name(&fomod_entry_name)
        .map_err(|e| format!("Cannot read fomod config: {e}"))?;
    let mut xml_bytes = Vec::new();
    entry
        .read_to_end(&mut xml_bytes)
        .map_err(|e| format!("Failed to read fomod config: {e}"))?;

    parse_fomod_xml(&xml_bytes)
}

fn find_fomod_config_in_dir(root: &Path) -> Option<std::path::PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) => {
                log::warn!("Failed to read extracted archive directory {}: {e}", dir.display());
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if !path.is_file() {
                continue;
            }
            let Ok(rel) = path.strip_prefix(root) else {
                continue;
            };
            let rel_lower = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
                .collect::<Vec<_>>()
                .join("/");
            if rel_lower.ends_with("fomod/moduleconfig.xml") {
                return Some(path);
            }
        }
    }
    None
}

fn find_fomod_entry(zip: &mut zip::ZipArchive<std::fs::File>) -> Result<String, String> {
    for i in 0..zip.len() {
        let entry = zip
            .by_index(i)
            .map_err(|e| format!("Cannot read zip entry: {e}"))?;
        let lower = entry.name().to_lowercase();
        if lower == "fomod/moduleconfig.xml" || lower.ends_with("/fomod/moduleconfig.xml") {
            return Ok(entry.name().to_string());
        }
    }
    Err("No fomod/ModuleConfig.xml found in archive".to_string())
}

/// Load a file from an archive by path, using case-insensitive matching and
/// common FOMOD-relative fallbacks.  Supports both zip archives and non-zip
/// archives (7z, rar, etc. — extracted via the `7z` command).
pub fn read_archive_file_bytes(
    archive_path: &Path,
    relative_path: &str,
) -> Result<Vec<u8>, String> {
    if !has_zip_extension(archive_path) {
        return read_archive_file_bytes_non_zip(archive_path, relative_path);
    }

    let file =
        std::fs::File::open(archive_path).map_err(|e| format!("Cannot open archive: {e}"))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| format!("Cannot read zip archive: {e}"))?;

    let target = normalise_path(relative_path);
    let target_lower = target.to_lowercase();
    if target_lower.is_empty() {
        return Err("Empty archive path".to_string());
    }
    let fomod_target = format!("{FOMOD_DIR_PREFIX}{target_lower}");

    // FOMOD image paths are often relative to `fomod/`, while some archives
    // store them at the root or inside a wrapped top-level folder. Match both
    // direct and `fomod/`-prefixed variants, including wrapped prefixes.
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| format!("Cannot read zip entry: {e}"))?;
        if entry.is_dir() {
            continue;
        }
        let name_norm = normalise_path(entry.name());
        let name_lower = name_norm.to_lowercase();
        let matches = name_lower == target_lower
            || name_lower.ends_with(&format!("/{target_lower}"))
            || name_lower == fomod_target
            || name_lower.ends_with(&format!("/{fomod_target}"));
        if matches {
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .map_err(|e| format!("Failed reading archive file {name_norm}: {e}"))?;
            return Ok(bytes);
        }
    }

    Err(format!("Archive file not found: {relative_path}"))
}

/// Read a file from a non-zip archive (7z, rar, etc.) by extracting to a
/// temporary directory and then searching for the target using the same
/// case-insensitive / `fomod/`-relative matching logic as the zip variant.
fn read_archive_file_bytes_non_zip(
    archive_path: &Path,
    relative_path: &str,
) -> Result<Vec<u8>, String> {
    let tmp = create_temp_extract_dir()?;
    let result = (|| {
        extract_archive_with_7z(archive_path, &tmp)?;

        let target = normalise_path(relative_path);
        let target_lower = target.to_lowercase();
        if target_lower.is_empty() {
            return Err("Empty archive path".to_string());
        }
        let fomod_target = format!("{FOMOD_DIR_PREFIX}{target_lower}");

        let mut files: Vec<(String, std::path::PathBuf)> = Vec::new();
        collect_fs_files(&tmp, &tmp, &mut files);

        for (rel_lower, full_path) in &files {
            let matches = *rel_lower == target_lower
                || rel_lower.ends_with(&format!("/{target_lower}"))
                || *rel_lower == fomod_target
                || rel_lower.ends_with(&format!("/{fomod_target}"));
            if matches {
                return std::fs::read(full_path)
                    .map_err(|e| format!("Failed to read {}: {e}", full_path.display()));
            }
        }

        Err(format!("Archive file not found: {relative_path}"))
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

/// Recursively collect all regular files under `dir`, returning tuples of
/// `(lowercase_relative_path, full_path)` relative to `root`.
fn collect_fs_files(
    root: &Path,
    dir: &Path,
    result: &mut Vec<(String, std::path::PathBuf)>,
) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_fs_files(root, &path, result);
        } else if path.is_file() {
            if let Ok(rel) = path.strip_prefix(root) {
                let rel_str = normalise_path(&rel.to_string_lossy());
                result.push((rel_str.to_lowercase(), path));
            }
        }
    }
}

/// Recursively collect all entries (files and directories) under `dir`,
/// returning tuples of `(lowercase_relative_path, original_relative_path)`
/// relative to `root`.
fn collect_fs_entries(
    root: &Path,
    dir: &Path,
    result: &mut Vec<(String, String)>,
) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let rel_str = normalise_path(&rel.to_string_lossy());
        if rel_str.is_empty() {
            continue;
        }
        result.push((rel_str.to_lowercase(), rel_str));
        if path.is_dir() {
            collect_fs_entries(root, &path, result);
        }
    }
}

/// Determine the common top-level prefix shared by all entries in a filesystem
/// entry map (mirrors [`find_common_prefix`] for zip archives).
///
/// Returns `(original_case_prefix, lowercase_prefix)`, both including a
/// trailing `/`, or two empty strings when no common prefix is found.
fn find_fs_common_prefix(entry_map: &[(String, String)]) -> (String, String) {
    if entry_map.len() <= 1 {
        return (String::new(), String::new());
    }
    let mut first_lower: Option<String> = None;
    let mut first_orig: Option<String> = None;
    let mut all_same = true;
    for (lower, orig) in entry_map {
        let tl = lower.split('/').next().unwrap_or("");
        let to = orig.split('/').next().unwrap_or("");
        if tl.is_empty() {
            continue;
        }
        match &first_lower {
            None => {
                first_lower = Some(tl.to_string());
                first_orig = Some(to.to_string());
            }
            Some(ft) if ft.as_str() != tl => {
                all_same = false;
                break;
            }
            _ => {}
        }
    }
    if all_same {
        if let (Some(tl), Some(to)) = (first_lower, first_orig) {
            return (format!("{to}/"), format!("{tl}/"));
        }
    }
    (String::new(), String::new())
}

/// Filter `entry_map` to entries whose lowercase relative path equals
/// `source_lower` or starts with `source_lower/`, returning the original
/// relative paths.
fn collect_matching_fs_entries(
    entry_map: &[(String, String)],
    source_lower: &str,
) -> Vec<String> {
    entry_map
        .iter()
        .filter(|(nl, _)| {
            *nl == source_lower
                || nl.starts_with(&format!("{source_lower}/"))
                || nl.starts_with(&format!("{source_lower}\\"))
        })
        .map(|(_, orig)| orig.clone())
        .collect()
}

fn parse_fomod_xml(xml_bytes: &[u8]) -> Result<FomodConfig, String> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let mut reader = Reader::from_reader(xml_bytes);
    reader.config_mut().trim_text(true);

    let mut config = FomodConfig {
        mod_name: None,
        required_files: Vec::new(),
        steps: Vec::new(),
        conditional_file_installs: Vec::new(),
    };

    let mut buf = Vec::new();
    let mut path_stack: Vec<String> = Vec::new();

    // Current parsing context
    let mut current_step: Option<InstallStep> = None;
    let mut current_group: Option<PluginGroup> = None;
    let mut current_plugin: Option<FomodPlugin> = None;
    let mut current_text = String::new();
    let mut current_condition_flag_name: Option<String> = None;
    let mut in_required = false;
    let mut in_visible = false;
    let mut in_pattern = false;
    let mut current_pattern_dependencies: Option<PluginDependencies> = None;
    let mut current_pattern_files: Vec<FomodFile> = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_lowercase();
                path_stack.push(tag.clone());

                match tag.as_str() {
                    "modulename" => {}
                    "requiredinstallfiles" => {
                        in_required = true;
                    }
                    "installstep" => {
                        let name = get_attr(e, "name").unwrap_or_default();
                        current_step = Some(InstallStep {
                            name,
                            visible: None,
                            groups: Vec::new(),
                        });
                    }
                    "visible" => {
                        in_visible = true;
                    }
                    "pattern" => {
                        in_pattern = true;
                        current_pattern_dependencies = None;
                        current_pattern_files.clear();
                    }
                    "group" => {
                        let name = get_attr(e, "name").unwrap_or_default();
                        let type_str =
                            get_attr(e, "type").unwrap_or_else(|| "SelectAny".to_string());
                        let group_type = match type_str.to_lowercase().as_str() {
                            "selectatleastone" => GroupType::SelectAtLeastOne,
                            "selectatmostone" => GroupType::SelectAtMostOne,
                            "selectexactlyone" => GroupType::SelectExactlyOne,
                            "selectall" => GroupType::SelectAll,
                            _ => GroupType::SelectAny,
                        };
                        current_group = Some(PluginGroup {
                            name,
                            group_type,
                            plugins: Vec::new(),
                        });
                    }
                    "plugin" => {
                        let name = get_attr(e, "name").unwrap_or_default();
                        current_plugin = Some(FomodPlugin {
                            name,
                            description: None,
                            image_path: None,
                            files: Vec::new(),
                            type_descriptor: PluginType::Optional,
                            condition_flags: Vec::new(),
                            dependencies: None,
                        });
                    }
                    "image" => {
                        if let Some(ref mut plugin) = current_plugin {
                            plugin.image_path = get_attr(e, "path");
                        }
                    }
                    "file" | "folder" => {
                        let source = get_attr(e, "source").unwrap_or_default();
                        let destination = get_attr(e, "destination").unwrap_or_default();
                        let priority = get_attr(e, "priority")
                            .and_then(|p| p.parse::<i32>().ok())
                            .unwrap_or(0);

                        let fomod_file = FomodFile {
                            source,
                            destination,
                            priority,
                        };

                        if in_pattern {
                            current_pattern_files.push(fomod_file);
                        } else if in_required && current_plugin.is_none() {
                            config.required_files.push(fomod_file);
                        } else if let Some(ref mut plugin) = current_plugin {
                            plugin.files.push(fomod_file);
                        }
                    }
                    "type" => {
                        if current_plugin.is_some() {
                            let name = get_attr(e, "name").unwrap_or_default();
                            if let Some(ref mut plugin) = current_plugin {
                                plugin.type_descriptor = match name.to_lowercase().as_str() {
                                    "required" => PluginType::Required,
                                    "recommended" => PluginType::Recommended,
                                    "notusable" | "couldbeusable" => PluginType::NotUsable,
                                    _ => PluginType::Optional,
                                };
                            }
                        }
                    }
                    "dependencies" => {
                        let operator = match get_attr(e, "operator")
                            .unwrap_or_else(|| "And".to_string())
                            .to_lowercase()
                            .as_str()
                        {
                            "or" => DependencyOperator::Or,
                            _ => DependencyOperator::And,
                        };
                        let deps = PluginDependencies {
                            operator,
                            flags: Vec::new(),
                        };
                        if in_pattern {
                            current_pattern_dependencies = Some(deps);
                        } else if in_visible {
                            if let Some(ref mut step) = current_step {
                                step.visible = Some(deps);
                            }
                        } else if let Some(ref mut plugin) = current_plugin {
                            plugin.dependencies = Some(deps);
                        }
                    }
                    "flagdependency" => {
                        let flag = get_attr(e, "flag").unwrap_or_default();
                        let value = get_attr(e, "value").unwrap_or_default();
                        if !flag.is_empty() {
                            if in_pattern {
                                if current_pattern_dependencies.is_none() {
                                    current_pattern_dependencies = Some(PluginDependencies {
                                        operator: DependencyOperator::And,
                                        flags: Vec::new(),
                                    });
                                }
                                if let Some(ref mut deps) = current_pattern_dependencies {
                                    deps.flags.push(FlagDependency { flag, value });
                                }
                            } else if in_visible {
                                if let Some(ref mut step) = current_step {
                                    if step.visible.is_none() {
                                        step.visible = Some(PluginDependencies {
                                            operator: DependencyOperator::And,
                                            flags: Vec::new(),
                                        });
                                    }
                                    if let Some(ref mut deps) = step.visible {
                                        deps.flags.push(FlagDependency { flag, value });
                                    }
                                }
                            } else if let Some(ref mut plugin) = current_plugin {
                                if let Some(ref mut deps) = plugin.dependencies {
                                    deps.flags.push(FlagDependency { flag, value });
                                }
                            }
                        }
                    }
                    "flag" => {
                        let in_condition_flags = path_stack.iter().any(|p| p == "conditionflags");
                        if in_condition_flags && current_plugin.is_some() {
                            current_condition_flag_name = get_attr(e, "name");
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_lowercase();
                match tag.as_str() {
                    "file" | "folder" => {
                        let source = get_attr(e, "source").unwrap_or_default();
                        let destination = get_attr(e, "destination").unwrap_or_default();
                        let priority = get_attr(e, "priority")
                            .and_then(|p| p.parse::<i32>().ok())
                            .unwrap_or(0);

                        let fomod_file = FomodFile {
                            source,
                            destination,
                            priority,
                        };

                        if in_pattern {
                            current_pattern_files.push(fomod_file);
                        } else if in_required && current_plugin.is_none() {
                            config.required_files.push(fomod_file);
                        } else if let Some(ref mut plugin) = current_plugin {
                            plugin.files.push(fomod_file);
                        }
                    }
                    "type" => {
                        if let Some(ref mut plugin) = current_plugin {
                            let name = get_attr(e, "name").unwrap_or_default();
                            plugin.type_descriptor = match name.to_lowercase().as_str() {
                                "required" => PluginType::Required,
                                "recommended" => PluginType::Recommended,
                                "notusable" | "couldbeusable" => PluginType::NotUsable,
                                _ => PluginType::Optional,
                            };
                        }
                    }
                    "dependencies" => {
                        let operator = match get_attr(e, "operator")
                            .unwrap_or_else(|| "And".to_string())
                            .to_lowercase()
                            .as_str()
                        {
                            "or" => DependencyOperator::Or,
                            _ => DependencyOperator::And,
                        };
                        let deps = PluginDependencies {
                            operator,
                            flags: Vec::new(),
                        };
                        if in_pattern {
                            current_pattern_dependencies = Some(deps);
                        } else if in_visible {
                            if let Some(ref mut step) = current_step {
                                step.visible = Some(deps);
                            }
                        } else if let Some(ref mut plugin) = current_plugin {
                            plugin.dependencies = Some(deps);
                        }
                    }
                    "image" => {
                        if let Some(ref mut plugin) = current_plugin {
                            plugin.image_path = get_attr(e, "path");
                        }
                    }
                    "flag" => {
                        let in_condition_flags = path_stack.iter().any(|p| p == "conditionflags");
                        if in_condition_flags {
                            if let Some(ref mut plugin) = current_plugin {
                                let name = get_attr(e, "name").unwrap_or_default();
                                let value = get_attr(e, "value").unwrap_or_default();
                                if !name.is_empty() && !value.is_empty() {
                                    plugin.condition_flags.push(ConditionFlag { name, value });
                                }
                            }
                        }
                    }
                    "flagdependency" => {
                        let flag = get_attr(e, "flag").unwrap_or_default();
                        let value = get_attr(e, "value").unwrap_or_default();
                        if !flag.is_empty() {
                            if in_pattern {
                                if current_pattern_dependencies.is_none() {
                                    current_pattern_dependencies = Some(PluginDependencies {
                                        operator: DependencyOperator::And,
                                        flags: Vec::new(),
                                    });
                                }
                                if let Some(ref mut deps) = current_pattern_dependencies {
                                    deps.flags.push(FlagDependency { flag, value });
                                }
                            } else if in_visible {
                                if let Some(ref mut step) = current_step {
                                    if step.visible.is_none() {
                                        step.visible = Some(PluginDependencies {
                                            operator: DependencyOperator::And,
                                            flags: Vec::new(),
                                        });
                                    }
                                    if let Some(ref mut deps) = step.visible {
                                        deps.flags.push(FlagDependency { flag, value });
                                    }
                                }
                            } else if let Some(ref mut plugin) = current_plugin {
                                if let Some(ref mut deps) = plugin.dependencies {
                                    deps.flags.push(FlagDependency { flag, value });
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) => {
                current_text = e.unescape().unwrap_or_default().to_string();
            }
            Ok(Event::End(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_lowercase();
                path_stack.pop();

                match tag.as_str() {
                    "modulename" => {
                        if !current_text.is_empty() {
                            config.mod_name = Some(current_text.clone());
                        }
                    }
                    "requiredinstallfiles" => {
                        in_required = false;
                    }
                    "description" => {
                        if let Some(ref mut plugin) = current_plugin {
                            if !current_text.is_empty() {
                                plugin.description = Some(current_text.clone());
                            }
                        }
                    }
                    "flag" => {
                        let in_condition_flags = path_stack
                            .last()
                            .map(|p| p == "conditionflags")
                            .unwrap_or(false);
                        if in_condition_flags {
                            if let Some(name) = current_condition_flag_name.take() {
                                if let Some(ref mut plugin) = current_plugin {
                                    if !current_text.is_empty() {
                                        plugin.condition_flags.push(ConditionFlag {
                                            name,
                                            value: current_text.clone(),
                                        });
                                    }
                                }
                            }
                        }
                    }
                    "dependencies" => {
                        if in_pattern {
                            if let Some(ref deps) = current_pattern_dependencies {
                                if deps.flags.is_empty() {
                                    current_pattern_dependencies = None;
                                }
                            }
                        } else if in_visible {
                            if let Some(ref mut step) = current_step {
                                if let Some(ref deps) = step.visible {
                                    if deps.flags.is_empty() {
                                        step.visible = None;
                                    }
                                }
                            }
                        } else if let Some(ref mut plugin) = current_plugin {
                            if let Some(ref deps) = plugin.dependencies {
                                if deps.flags.is_empty() {
                                    plugin.dependencies = None;
                                }
                            }
                        }
                    }
                    "visible" => {
                        in_visible = false;
                    }
                    "plugin" => {
                        if let Some(plugin) = current_plugin.take() {
                            if let Some(ref mut group) = current_group {
                                group.plugins.push(plugin);
                            }
                        }
                    }
                    "group" => {
                        if let Some(group) = current_group.take() {
                            if let Some(ref mut step) = current_step {
                                step.groups.push(group);
                            }
                        }
                    }
                    "installstep" => {
                        if let Some(step) = current_step.take() {
                            config.steps.push(step);
                        }
                    }
                    "pattern" => {
                        let dependencies = current_pattern_dependencies.take().unwrap_or(
                            PluginDependencies {
                                operator: DependencyOperator::And,
                                flags: Vec::new(),
                            },
                        );
                        if !current_pattern_files.is_empty() {
                            config
                                .conditional_file_installs
                                .push(ConditionalFileInstall {
                                    dependencies,
                                    files: std::mem::take(&mut current_pattern_files),
                                });
                        } else {
                            current_pattern_files.clear();
                        }
                        in_pattern = false;
                    }
                    _ => {}
                }
                current_text.clear();
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(format!("XML parse error: {e}"));
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(config)
}

fn get_attr(event: &quick_xml::events::BytesStart<'_>, name: &str) -> Option<String> {
    for attr in event.attributes().flatten() {
        if attr.key.as_ref() == name.as_bytes() {
            return Some(attr.unescape_value().unwrap_or_default().to_string());
        }
    }
    None
}

// ── Installation logic ────────────────────────────────────────────────────────

/// Install a mod from a zip archive.
///
/// 1. Extracts the archive into `<mod_dir>/Data/` so that the managed directory
///    always uses the `{uuid}/Data/…` structure.
/// 2. Updates the mod database.
/// 3. Returns the created `Mod` entry.
pub fn install_mod_from_archive(
    archive_path: &Path,
    game: &Game,
    mod_name: &str,
    strategy: &InstallStrategy,
) -> Result<Mod, String> {
    install_mod_from_archive_with_nexus(archive_path, game, mod_name, strategy, None)
}

pub fn install_mod_from_archive_with_nexus(
    archive_path: &Path,
    game: &Game,
    mod_name: &str,
    strategy: &InstallStrategy,
    nexus_id: Option<u32>,
) -> Result<Mod, String> {
    let mod_dir = ModManager::create_mod_directory(game)?;

    match strategy {
        InstallStrategy::Data => {
            if !has_zip_extension(archive_path) {
                install_data_archive_non_zip(archive_path, &mod_dir)?;
            } else {
            // Determine extraction root:
            // • If the archive already carries a `Data/` subfolder after the
            //   common wrapper prefix is stripped, extract to `mod_dir/`
            //   directly – the `Data/` folder will land at `mod_dir/Data/`.
            // • Otherwise wrap the content inside `mod_dir/Data/` ourselves.
                if archive_has_data_folder(archive_path) {
                    extract_zip_to(archive_path, &mod_dir)?;
                } else {
                    let data_dir = mod_dir.join("Data");
                    std::fs::create_dir_all(&data_dir)
                        .map_err(|e| format!("Failed to create Data directory: {e}"))?;
                    extract_zip_to(archive_path, &data_dir)?;
                }
            }
        }
        InstallStrategy::Fomod(files) => {
            // FOMOD destinations are relative to the game's Data folder, so
            // extract directly into `mod_dir/Data/`.
            let data_dir = mod_dir.join("Data");
            std::fs::create_dir_all(&data_dir)
                .map_err(|e| format!("Failed to create Data directory: {e}"))?;
            install_fomod_files(archive_path, &data_dir, files)?;
        }
    }

    let mut mod_entry = Mod::new(mod_name, mod_dir);
    mod_entry.installed_from_nexus = nexus_id.is_some();
    mod_entry.nexus_id = nexus_id;

    // Register in the mod database
    let mut db = ModDatabase::load(game);
    // Avoid duplicates
    db.mods.retain(|m| m.name != mod_name);
    db.mods.push(mod_entry.clone());
    db.save(game);

    Ok(mod_entry)
}

/// Extract all files from a zip archive into `dest_dir`, preserving directory
/// structure.
fn extract_zip_to(archive_path: &Path, dest_dir: &Path) -> Result<(), String> {
    let file =
        std::fs::File::open(archive_path).map_err(|e| format!("Cannot open archive: {e}"))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| format!("Cannot read zip archive: {e}"))?;

    // Find common prefix to strip (e.g. when archive has a single top-level
    // folder wrapping everything).  Uses file_names() which reads only the
    // central-directory metadata – no decompression – so we can keep a single
    // file handle for both the prefix scan and the extraction pass.
    let prefix = find_common_prefix(&zip);

    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| format!("Cannot read zip entry: {e}"))?;

        let raw_name = entry.name().to_string();
        // Strip common prefix
        let rel_name = if !prefix.is_empty() {
            raw_name
                .strip_prefix(&prefix)
                .unwrap_or(&raw_name)
                .to_string()
        } else {
            raw_name
        };

        if rel_name.is_empty() || rel_name == "/" {
            continue;
        }

        // Zip-slip protection: reject entries with path traversal components
        if !is_safe_relative_path(&rel_name) {
            log::warn!("Skipping zip entry with unsafe path: {rel_name}");
            continue;
        }

        let out_path = dest_dir.join(&rel_name);

        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)
                .map_err(|e| format!("Failed to create directory {}: {e}", out_path.display()))?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create parent dir: {e}"))?;
            }
            let out_file = std::fs::File::create(&out_path)
                .map_err(|e| format!("Failed to create file {}: {e}", out_path.display()))?;
            let mut buffered = BufWriter::with_capacity(256 * 1024, out_file);
            std::io::copy(&mut entry, &mut buffered)
                .map_err(|e| format!("Failed to extract {}: {e}", rel_name))?;
        }
    }

    Ok(())
}

fn has_zip_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("zip"))
        .unwrap_or(false)
}

fn extract_archive_with_7z(archive_path: &Path, dest_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest_dir).map_err(|e| {
        format!(
            "Failed to create extraction directory {}: {e}",
            dest_dir.display()
        )
    })?;
    let output_arg = format!("-o{}", dest_dir.display());
    let output = Command::new("7z")
        .arg("x")
        .arg("-y")
        .arg(output_arg)
        .arg(archive_path)
        .output()
        .map_err(|e| {
            format!(
                "Failed to start 7z extractor ({e}). Install 7-Zip (the '7z' command) to extract .7z and .rar archives."
            )
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(format!(
            "Failed to extract non-zip archive with 7z. stdout: {stdout}; stderr: {stderr}"
        ));
    }
    Ok(())
}

fn install_data_archive_non_zip(archive_path: &Path, mod_dir: &Path) -> Result<(), String> {
    let tmp_extract = create_temp_extract_dir()?;
    extract_archive_with_7z(archive_path, &tmp_extract)?;

    let mut top_dirs = Vec::new();
    let mut top_files = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&tmp_extract) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                top_dirs.push(path);
            } else if path.is_file() {
                top_files.push(path);
            }
        }
    }

    let data_source = if tmp_extract.join("Data").is_dir() {
        Some(tmp_extract.clone())
    } else if top_dirs.len() == 1 && top_files.is_empty() && top_dirs[0].join("Data").is_dir() {
        Some(top_dirs[0].clone())
    } else {
        None
    };

    if let Some(source_root) = data_source {
        copy_dir_contents(&source_root, mod_dir)?;
    } else {
        let data_dir = mod_dir.join("Data");
        std::fs::create_dir_all(&data_dir)
            .map_err(|e| format!("Failed to create Data directory: {e}"))?;
        copy_dir_contents(&tmp_extract, &data_dir)?;
    }

    if let Err(e) = std::fs::remove_dir_all(&tmp_extract) {
        log::warn!(
            "Failed to remove temporary extraction directory {}: {e}",
            tmp_extract.display()
        );
    }
    Ok(())
}

fn create_temp_extract_dir() -> Result<std::path::PathBuf, String> {
    let base = std::env::temp_dir();
    for attempt in 0..100u32 {
        let temp_extract_path = base.join(format!(
            "linkmm_extract_{}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            attempt
        ));
        match std::fs::create_dir(&temp_extract_path) {
            Ok(()) => return Ok(temp_extract_path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(format!(
                    "Failed to create temporary extraction directory {}: {e}",
                    temp_extract_path.display()
                ));
            }
        }
    }
    Err("Failed to allocate a temporary extraction directory".to_string())
}

fn copy_dir_contents(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst)
        .map_err(|e| format!("Failed to create destination {}: {e}", dst.display()))?;
    for entry in std::fs::read_dir(src).map_err(|e| format!("Failed to read {}: {e}", src.display()))? {
        let entry = entry.map_err(|e| format!("Failed to read directory entry: {e}"))?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_contents(&from, &to)?;
        } else if from.is_file() {
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create parent dir {}: {e}", parent.display()))?;
            }
            std::fs::copy(&from, &to).map_err(|e| {
                format!(
                    "Failed to copy extracted file {} -> {}: {e}",
                    from.display(),
                    to.display()
                )
            })?;
        }
    }
    Ok(())
}

/// Detect whether all entries in the archive share a common top-level directory
/// prefix.  If so, return it (with trailing `/`).
///
/// Uses `ZipArchive::file_names()` which only reads the central directory
/// metadata (no decompression), so this is fast and does not require a
/// mutable borrow.
fn find_common_prefix(zip: &zip::ZipArchive<std::fs::File>) -> String {
    if zip.is_empty() {
        return String::new();
    }

    let mut first_top: Option<String> = None;
    let mut all_same = true;

    for name in zip.file_names() {
        let top = name.split('/').next().unwrap_or("");
        if top.is_empty() {
            continue;
        }
        match &first_top {
            None => first_top = Some(top.to_string()),
            Some(ft) if ft != top => {
                all_same = false;
                break;
            }
            _ => {}
        }
    }

    if all_same {
        if let Some(ft) = first_top {
            // Only strip if there are entries *inside* the folder (not just the folder itself)
            if zip.len() > 1 {
                return format!("{ft}/");
            }
        }
    }
    String::new()
}

/// Install FOMOD-selected files from an archive.
///
/// Supports both zip archives and non-zip archives (7z, rar, etc.).  For
/// non-zip archives the archive is first fully extracted to a temporary
/// directory; the per-file selection logic is then applied to the extracted
/// tree before the temp directory is removed.
///
/// For each `FomodFile`, extract the `source` path from the archive and place it
/// at `dest_dir / destination`.
fn install_fomod_files(
    archive_path: &Path,
    dest_dir: &Path,
    files: &[FomodFile],
) -> Result<(), String> {
    if !has_zip_extension(archive_path) {
        return install_fomod_files_non_zip(archive_path, dest_dir, files);
    }
    let file =
        std::fs::File::open(archive_path).map_err(|e| format!("Cannot open archive: {e}"))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| format!("Cannot read zip archive: {e}"))?;
    let archive_prefix = normalise_path(&find_common_prefix(&zip));
    let archive_prefix_lower = archive_prefix.to_lowercase();

    // Build a map of lowercased entry names → indices for case-insensitive
    // matching.
    let mut entry_map: Vec<(String, String, usize)> = Vec::new(); // (lower, original, index)
    for i in 0..zip.len() {
        let Ok(entry) = zip.by_index(i) else {
            continue;
        };
        let name = entry.name().to_string();
        let lower = name.to_lowercase();
        entry_map.push((lower, name, i));
    }

    // Sort files by priority (higher priority wins for same destination)
    let mut sorted_files = files.to_vec();
    sorted_files.sort_by(|a, b| a.priority.cmp(&b.priority));

    for fomod_file in &sorted_files {
        let source = normalise_path(&fomod_file.source);
        // Strip a leading "Data/" segment: FOMOD destinations are relative to
        // the game root, but dest_dir is already mod_dir/Data/.  Without this
        // stripping a destination of "Data/textures" would produce the double-
        // nested mod_dir/Data/Data/textures/ layout.
        let destination = strip_data_prefix(&normalise_path(&fomod_file.destination));
        let source_lower = source.to_lowercase();

        // Find matching entry indices. Some FOMODs reference files under
        // "Data/..." while the archive has the same files at root (or vice
        // versa), so try fallback source aliases.
        let mut matched_source = source.clone();
        let mut matching_indices = collect_matching_entries(&entry_map, &source_lower);

        if matching_indices.is_empty() {
            if !archive_prefix_lower.is_empty() {
                let wrapped_source_lower = format!("{archive_prefix_lower}/{source_lower}");
                matching_indices = collect_matching_entries(&entry_map, &wrapped_source_lower);
                if !matching_indices.is_empty() {
                    matched_source = format!("{archive_prefix}/{source}");
                }
            }
        }

        if matching_indices.is_empty() {
            let stripped_source = strip_data_prefix(&source);
            let stripped_lower = stripped_source.to_lowercase();
            if !stripped_source.is_empty() && stripped_lower != source_lower {
                matching_indices = collect_matching_entries(&entry_map, &stripped_lower);
                if !matching_indices.is_empty() {
                    matched_source = stripped_source;
                } else if !archive_prefix_lower.is_empty() {
                    let wrapped_stripped_lower = format!("{archive_prefix_lower}/{stripped_lower}");
                    matching_indices = collect_matching_entries(&entry_map, &wrapped_stripped_lower);
                    if !matching_indices.is_empty() {
                        matched_source = format!("{archive_prefix}/{stripped_source}");
                    }
                }
            } else if source_lower == "data" || source_lower == "data/" {
                matching_indices = entry_map
                    .iter()
                    .filter(|(nl, _, _)| {
                        !(nl == "fomod" || nl.starts_with("fomod/") || nl.contains("/fomod/"))
                    })
                    .map(|(_, orig, idx)| (orig.clone(), *idx))
                    .collect();
                if !matching_indices.is_empty() {
                    matched_source = String::new();
                }
            }
        }

        if matching_indices.is_empty() && !source_lower.is_empty() {
            let prefixed = format!("data/{source_lower}");
            matching_indices = collect_matching_entries(&entry_map, &prefixed);
            if !matching_indices.is_empty() {
                matched_source = prefixed;
            } else if !archive_prefix_lower.is_empty() {
                let wrapped_prefixed = format!("{archive_prefix_lower}/{prefixed}");
                matching_indices = collect_matching_entries(&entry_map, &wrapped_prefixed);
                if !matching_indices.is_empty() {
                    matched_source = format!("{archive_prefix}/data/{source}");
                }
            }
        }

        let matched_source_lower = matched_source.to_lowercase();

        for (entry_name, entry_idx) in matching_indices {
            let entry_lower = entry_name.to_lowercase();
            // Compute the relative portion after the source prefix
            let rel = if entry_lower == matched_source_lower {
                // Single file
                Path::new(&entry_name)
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_default()
            } else {
                entry_name[matched_source.len()..]
                    .trim_start_matches('/')
                    .to_string()
            };

            if rel.is_empty() {
                continue;
            }

            // Zip-slip protection on the combined destination + rel path
            let combined = if destination.is_empty() {
                std::borrow::Cow::Borrowed(rel.as_str())
            } else {
                std::borrow::Cow::Owned(format!("{destination}/{rel}"))
            };
            if !is_safe_relative_path(combined.as_ref()) {
                log::warn!("Skipping fomod entry with unsafe path: {combined}");
                continue;
            }

            let out_path = dest_dir.join(&destination).join(&rel);

            // Use by_index to avoid long-lived borrow issues
            let mut entry = zip
                .by_index(entry_idx)
                .map_err(|e| format!("Cannot read entry {entry_name}: {e}"))?;

            if entry.is_dir() {
                continue;
            }

            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create parent dir: {e}"))?;
            }

            let out_file =
                std::fs::File::create(&out_path).map_err(|e| format!("Failed to create file: {e}"))?;
            let mut buffered = BufWriter::with_capacity(256 * 1024, out_file);
            std::io::copy(&mut entry, &mut buffered).map_err(|e| format!("Failed to extract: {e}"))?;
        }
    }

    Ok(())
}

/// Extract a non-zip archive to a temporary directory, run the FOMOD file
/// selection logic on the extracted tree, then remove the temp directory.
fn install_fomod_files_non_zip(
    archive_path: &Path,
    dest_dir: &Path,
    files: &[FomodFile],
) -> Result<(), String> {
    let tmp = create_temp_extract_dir()?;
    extract_archive_with_7z(archive_path, &tmp)?;
    let result = install_fomod_files_from_dir(&tmp, dest_dir, files);
    if let Err(e) = std::fs::remove_dir_all(&tmp) {
        log::warn!(
            "Failed to remove temporary extraction directory {}: {e}",
            tmp.display()
        );
    }
    result
}

/// Install FOMOD-selected files from an already-extracted directory tree.
///
/// Mirrors the matching and extraction logic of [`install_fomod_files`] but
/// operates on real filesystem paths instead of a zip archive.
fn install_fomod_files_from_dir(
    extracted_dir: &Path,
    dest_dir: &Path,
    files: &[FomodFile],
) -> Result<(), String> {
    // Build a map of (lowercase_rel, original_rel) for all entries in the tree.
    let mut entry_map: Vec<(String, String)> = Vec::new();
    collect_fs_entries(extracted_dir, extracted_dir, &mut entry_map);

    let (archive_prefix, archive_prefix_lower) = find_fs_common_prefix(&entry_map);

    let mut sorted_files = files.to_vec();
    sorted_files.sort_by(|a, b| a.priority.cmp(&b.priority));

    for fomod_file in &sorted_files {
        let source = normalise_path(&fomod_file.source);
        let destination = strip_data_prefix(&normalise_path(&fomod_file.destination));
        let source_lower = source.to_lowercase();

        let mut matched_source = source.clone();
        let mut matching_rels = collect_matching_fs_entries(&entry_map, &source_lower);

        if matching_rels.is_empty() && !archive_prefix_lower.is_empty() {
            let wrapped = format!("{archive_prefix_lower}{source_lower}");
            matching_rels = collect_matching_fs_entries(&entry_map, &wrapped);
            if !matching_rels.is_empty() {
                matched_source = format!("{archive_prefix}{source}");
            }
        }

        if matching_rels.is_empty() {
            let stripped = strip_data_prefix(&source);
            let stripped_lower = stripped.to_lowercase();
            if !stripped.is_empty() && stripped_lower != source_lower {
                matching_rels = collect_matching_fs_entries(&entry_map, &stripped_lower);
                if !matching_rels.is_empty() {
                    matched_source = stripped;
                } else if !archive_prefix_lower.is_empty() {
                    let wrapped_stripped = format!("{archive_prefix_lower}{stripped_lower}");
                    matching_rels = collect_matching_fs_entries(&entry_map, &wrapped_stripped);
                    if !matching_rels.is_empty() {
                        matched_source = format!("{archive_prefix}{stripped}");
                    }
                }
            } else if source_lower == "data" || source_lower == "data/" {
                matching_rels = entry_map
                    .iter()
                    .filter(|(nl, _)| {
                        !(nl == "fomod"
                            || nl.starts_with("fomod/")
                            || nl.contains("/fomod/"))
                    })
                    .map(|(_, orig)| orig.clone())
                    .collect();
                if !matching_rels.is_empty() {
                    matched_source = String::new();
                }
            }
        }

        if matching_rels.is_empty() && !source_lower.is_empty() {
            let prefixed = format!("data/{source_lower}");
            matching_rels = collect_matching_fs_entries(&entry_map, &prefixed);
            if !matching_rels.is_empty() {
                matched_source = prefixed;
            } else if !archive_prefix_lower.is_empty() {
                let wrapped_prefixed = format!("{archive_prefix_lower}data/{source_lower}");
                matching_rels = collect_matching_fs_entries(&entry_map, &wrapped_prefixed);
                if !matching_rels.is_empty() {
                    matched_source = format!("{archive_prefix}data/{source}");
                }
            }
        }

        let matched_source_lower = matched_source.to_lowercase();

        for orig_rel in matching_rels {
            let full_path = extracted_dir.join(&orig_rel);

            // Skip directory entries – only copy actual files.
            if full_path.is_dir() {
                continue;
            }

            let entry_lower = orig_rel.to_lowercase();
            let rel = if entry_lower == matched_source_lower {
                // Single-file match: use the file name only.
                Path::new(&orig_rel)
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_default()
            } else {
                orig_rel[matched_source.len()..]
                    .trim_start_matches('/')
                    .to_string()
            };

            if rel.is_empty() {
                continue;
            }

            // Zip-slip protection on the combined destination + rel path.
            let combined = if destination.is_empty() {
                std::borrow::Cow::Borrowed(rel.as_str())
            } else {
                std::borrow::Cow::Owned(format!("{destination}/{rel}"))
            };
            if !is_safe_relative_path(combined.as_ref()) {
                log::warn!("Skipping fomod entry with unsafe path: {combined}");
                continue;
            }

            let out_path = dest_dir.join(&destination).join(&rel);

            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create parent dir: {e}"))?;
            }

            std::fs::copy(&full_path, &out_path).map_err(|e| {
                format!(
                    "Failed to copy {} → {}: {e}",
                    full_path.display(),
                    out_path.display()
                )
            })?;
        }
    }

    Ok(())
}

fn normalise_path(p: &str) -> String {
    let s = p.replace('\\', "/");
    let s = s.strip_prefix('/').unwrap_or(&s);
    s.trim_end_matches('/').to_string()
}

fn collect_matching_entries(
    entry_map: &[(String, String, usize)],
    source_lower: &str,
) -> Vec<(String, usize)> {
    entry_map
        .iter()
        .filter(|(nl, _, _)| {
            *nl == source_lower
                || nl.starts_with(&format!("{source_lower}/"))
                || nl.starts_with(&format!("{source_lower}\\"))
        })
        .map(|(_, orig, idx)| (orig.clone(), *idx))
        .collect()
}

/// Strip a leading `Data/` segment (case-insensitive) from a FOMOD destination
/// path.
///
/// FOMOD `destination` attributes are relative to the **game root**, so a value
/// of `"Data/textures"` means the game's `Data/textures` folder.  Because
/// [`install_fomod_files`] already extracts into `mod_dir/Data/`, including the
/// `Data/` segment verbatim would produce `mod_dir/Data/Data/textures/` — the
/// classic double-nesting bug.  Stripping the leading `Data/` (or bare `Data`)
/// avoids this.
fn strip_data_prefix(s: &str) -> String {
    let lower = s.to_lowercase();
    if lower == "data" || lower == "data/" {
        String::new()
    } else if lower.starts_with("data/") {
        s["data/".len()..].to_string()
    } else {
        s.to_string()
    }
}

/// Check that a relative path is safe (no traversal above the root).
///
/// Rejects paths containing `..` components that would escape the destination.
fn is_safe_relative_path(path: &str) -> bool {
    use std::path::Component;
    let normalised = path.replace('\\', "/");
    let p = Path::new(&normalised);
    let mut depth: i32 = 0;
    for component in p.components() {
        match component {
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            Component::Normal(_) => {
                depth += 1;
            }
            Component::RootDir | Component::Prefix(_) => {
                // Absolute paths are not safe relative paths
                return false;
            }
            Component::CurDir => {}
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    /// Create a simple zip archive in `dir` containing the given entries.
    /// Each entry is (name, content).  Names ending with `/` are directories.
    fn create_test_zip(dir: &Path, entries: &[(&str, &[u8])]) -> PathBuf {
        let archive_path = dir.join("test_mod.zip");
        let file = std::fs::File::create(&archive_path).unwrap();
        let mut zip_writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for &(name, content) in entries {
            if name.ends_with('/') {
                zip_writer.add_directory(name, options).unwrap();
            } else {
                zip_writer.start_file(name, options).unwrap();
                zip_writer.write_all(content).unwrap();
            }
        }
        // finish() returns the inner File – drop it to flush
        let inner = zip_writer.finish().unwrap();
        drop(inner);
        archive_path
    }

    fn has_7z() -> bool {
        Command::new("7z").arg("--help").output().is_ok()
    }

    fn create_test_7z(root: &Path, archive_name: &str) -> Option<PathBuf> {
        if !has_7z() {
            return None;
        }
        let archive_path = root.join(archive_name);
        let staging = root.join("staging");
        if let Err(e) = std::fs::create_dir_all(staging.join("fomod")) {
            eprintln!("create_test_7z: failed to create fomod dir: {e}");
            return None;
        }
        if let Err(e) = std::fs::create_dir_all(staging.join("Data/textures")) {
            eprintln!("create_test_7z: failed to create Data/textures dir: {e}");
            return None;
        }
        if let Err(e) = std::fs::write(
            staging.join("fomod/ModuleConfig.xml"),
            r#"<config>
  <requiredInstallFiles>
    <file source="Data/textures/sky.dds" destination="Data/textures/sky.dds" />
  </requiredInstallFiles>
</config>"#,
        ) {
            eprintln!("create_test_7z: failed to write ModuleConfig.xml: {e}");
            return None;
        }
        if let Err(e) = std::fs::write(staging.join("Data/textures/sky.dds"), "dds") {
            eprintln!("create_test_7z: failed to write test data file: {e}");
            return None;
        }
        let output = Command::new("7z")
            .current_dir(&staging)
            .arg("a")
            .arg("-y")
            .arg(&archive_path)
            .arg(".")
            .output();
        let output = match output {
            Ok(output) => output,
            Err(e) => {
                eprintln!("create_test_7z: failed to launch 7z: {e}");
                return None;
            }
        };
        if output.status.success() {
            Some(archive_path)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("7z test archive creation failed: {stderr}");
            None
        }
    }

    /// Create a 7z archive containing a FOMOD image for testing.
    ///
    /// Layout:
    /// ```
    /// fomod/images/preview.png  (content: "pngdata")
    /// Data/textures/sky.dds
    /// ```
    fn create_test_7z_with_image(root: &Path, archive_name: &str) -> Option<PathBuf> {
        if !has_7z() {
            return None;
        }
        let archive_path = root.join(archive_name);
        let staging = root.join("staging_img");
        if let Err(e) = std::fs::create_dir_all(staging.join("fomod/images")) {
            eprintln!("create_test_7z_with_image: {e}");
            return None;
        }
        if let Err(e) = std::fs::create_dir_all(staging.join("Data/textures")) {
            eprintln!("create_test_7z_with_image: {e}");
            return None;
        }
        if let Err(e) = std::fs::write(staging.join("fomod/images/preview.png"), b"pngdata") {
            eprintln!("create_test_7z_with_image: {e}");
            return None;
        }
        if let Err(e) = std::fs::write(staging.join("Data/textures/sky.dds"), b"dds") {
            eprintln!("create_test_7z_with_image: {e}");
            return None;
        }
        let output = Command::new("7z")
            .current_dir(&staging)
            .arg("a")
            .arg("-y")
            .arg(&archive_path)
            .arg(".")
            .output();
        match output {
            Ok(o) if o.status.success() => Some(archive_path),
            Ok(o) => {
                eprintln!(
                    "7z image archive creation failed: {}",
                    String::from_utf8_lossy(&o.stderr)
                );
                None
            }
            Err(e) => {
                eprintln!("create_test_7z_with_image: failed to launch 7z: {e}");
                None
            }
        }
    }

    #[test]
    fn detect_strategy_data_for_loose_files() {
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[("textures/sky.dds", b"dds"), ("meshes/rock.nif", b"nif")],
        );
        let strategy = detect_strategy(&archive).unwrap();
        assert!(matches!(strategy, InstallStrategy::Data));
    }

    #[test]
    fn detect_strategy_data_for_archive_with_data_folder() {
        // Archives that already contain a Data/ subfolder now also return Data,
        // not Root – the distinction is handled during extraction.
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[
                ("Data/", b""),
                ("Data/textures/sky.dds", b"dds"),
                ("Data/meshes/rock.nif", b"nif"),
            ],
        );
        let strategy = detect_strategy(&archive).unwrap();
        assert!(matches!(strategy, InstallStrategy::Data));
    }

    #[test]
    fn detect_strategy_non_zip_defaults_to_data() {
        let tmp = tempdir();
        let archive = tmp.join("mod.7z");
        std::fs::write(&archive, b"fake").unwrap();
        let strategy = detect_strategy(&archive).unwrap();
        assert!(matches!(strategy, InstallStrategy::Data));
    }

    #[test]
    fn detect_strategy_non_zip_with_fomod_uses_fomod_strategy() {
        let tmp = tempdir();
        let Some(archive) = create_test_7z(&tmp, "mod.7z") else {
            return;
        };
        let strategy = detect_strategy(&archive).unwrap();
        assert!(matches!(strategy, InstallStrategy::Fomod(_)));
    }

    #[test]
    fn parse_fomod_from_archive_reads_non_zip_module_config() {
        let tmp = tempdir();
        let Some(archive) = create_test_7z(&tmp, "mod.7z") else {
            return;
        };
        let cfg = parse_fomod_from_archive(&archive).unwrap();
        assert!(!cfg.required_files.is_empty());
    }

    #[test]
    fn archive_has_data_folder_flat_content() {
        // Archive without Data/ subfolder – helper returns false.
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[("textures/sky.dds", b"dds"), ("plugin.esp", b"esp")],
        );
        assert!(!archive_has_data_folder(&archive));
    }

    #[test]
    fn archive_has_data_folder_direct_data_prefix() {
        // Archive where Data/ is the *only* top-level folder: find_common_prefix
        // strips it, so after stripping the remaining entries no longer start
        // with Data/ → helper returns false → content is extracted into Data/.
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[
                ("Data/", b""),
                ("Data/textures/sky.dds", b"dds"),
                ("Data/meshes/rock.nif", b"nif"),
            ],
        );
        assert!(!archive_has_data_folder(&archive));
    }

    #[test]
    fn archive_has_data_folder_wrapped_data() {
        // Archive with a wrapper folder containing Data/ – after stripping the
        // wrapper the remaining path starts with Data/ → helper returns true.
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[
                ("MyMod/", b""),
                ("MyMod/Data/", b""),
                ("MyMod/Data/textures/sky.dds", b"dds"),
            ],
        );
        assert!(archive_has_data_folder(&archive));
    }

    #[test]
    fn install_flat_archive_places_files_under_data_subdir() {
        // Flat archive (textures/sky.dds) should land in dest/Data/textures/sky.dds
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[
                ("textures/sky.dds", b"dds_data"),
                ("plugin.esp", b"esp_data"),
            ],
        );
        let dest = tmp.join("mod_dir");
        std::fs::create_dir_all(&dest).unwrap();
        let data_dest = dest.join("Data");
        std::fs::create_dir_all(&data_dest).unwrap();
        extract_zip_to(&archive, &data_dest).unwrap();

        assert!(dest.join("Data").join("textures").join("sky.dds").exists());
        assert!(dest.join("Data").join("plugin.esp").exists());
        assert_eq!(
            std::fs::read_to_string(dest.join("Data").join("plugin.esp")).unwrap(),
            "esp_data"
        );
    }

    #[test]
    fn install_data_folder_archive_preserves_data_subdir() {
        // Archive Data/textures/sky.dds: find_common_prefix strips Data/ so
        // extraction to mod_dir/Data/ gives mod_dir/Data/textures/sky.dds.
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[("Data/", b""), ("Data/textures/sky.dds", b"dds_data")],
        );
        let dest = tmp.join("mod_dir");
        std::fs::create_dir_all(&dest).unwrap();
        // archive_has_data_folder returns false for this archive (Data/ stripped)
        // so installer extracts to dest/Data/.
        assert!(!archive_has_data_folder(&archive));
        let data_dest = dest.join("Data");
        std::fs::create_dir_all(&data_dest).unwrap();
        extract_zip_to(&archive, &data_dest).unwrap();

        assert!(dest.join("Data").join("textures").join("sky.dds").exists());
    }

    #[test]
    fn install_wrapped_data_archive_preserves_data_subdir() {
        // Archive MyMod/Data/textures/sky.dds: prefix MyMod/ stripped, remaining
        // starts with Data/ → extract to mod_dir/ → mod_dir/Data/textures/sky.dds.
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[
                ("MyMod/", b""),
                ("MyMod/Data/", b""),
                ("MyMod/Data/textures/sky.dds", b"dds_data"),
            ],
        );
        let dest = tmp.join("mod_dir");
        std::fs::create_dir_all(&dest).unwrap();
        assert!(archive_has_data_folder(&archive));
        extract_zip_to(&archive, &dest).unwrap();

        assert!(dest.join("Data").join("textures").join("sky.dds").exists());
    }

    #[test]
    fn extract_zip_strips_common_prefix() {
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[
                ("MyMod/", b""),
                ("MyMod/textures/sky.dds", b"dds_data"),
                ("MyMod/plugin.esp", b"esp_data"),
            ],
        );
        let dest = tmp.join("extracted");
        std::fs::create_dir_all(&dest).unwrap();
        extract_zip_to(&archive, &dest).unwrap();

        // Common prefix "MyMod/" is stripped
        assert!(dest.join("textures").join("sky.dds").exists());
        assert!(dest.join("plugin.esp").exists());
    }

    #[test]
    fn is_safe_relative_path_rejects_traversal() {
        assert!(!is_safe_relative_path("../etc/passwd"));
        assert!(!is_safe_relative_path("foo/../../bar"));
        assert!(!is_safe_relative_path("/absolute/path"));
        assert!(is_safe_relative_path("foo/bar/baz"));
        assert!(is_safe_relative_path("textures/sky.dds"));
        assert!(is_safe_relative_path("a/../a/b")); // depth never goes negative
    }

    #[test]
    fn strip_data_prefix_removes_leading_data_segment() {
        // Bare "Data" with various casings / trailing slashes.
        assert_eq!(strip_data_prefix("Data"), "");
        assert_eq!(strip_data_prefix("data"), "");
        assert_eq!(strip_data_prefix("DATA"), "");
        assert_eq!(strip_data_prefix("Data/"), "");
        // Leading "Data/" prefix is stripped; rest is preserved.
        assert_eq!(strip_data_prefix("Data/textures"), "textures");
        assert_eq!(strip_data_prefix("data/Textures/sky"), "Textures/sky");
        assert_eq!(strip_data_prefix("DATA/meshes/rock.nif"), "meshes/rock.nif");
        // Non-Data paths are returned unchanged.
        assert_eq!(strip_data_prefix("textures/sky.dds"), "textures/sky.dds");
        assert_eq!(strip_data_prefix(""), "");
        assert_eq!(strip_data_prefix("SomeOtherFolder"), "SomeOtherFolder");
    }

    #[test]
    fn install_fomod_files_falls_back_when_source_uses_data_prefix() {
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[
                ("fomod/", b""),
                ("fomod/ModuleConfig.xml", b"<config/>"),
                ("textures/sky.dds", b"dds_data"),
            ],
        );
        let dest = tmp.join("mod_data");
        std::fs::create_dir_all(&dest).unwrap();

        install_fomod_files(
            &archive,
            &dest,
            &[FomodFile {
                source: "Data/textures".to_string(),
                destination: "Data/textures".to_string(),
                priority: 0,
            }],
        )
        .unwrap();

        assert!(dest.join("textures").join("sky.dds").exists());
    }

    #[test]
    fn install_fomod_files_handles_source_with_trailing_slash() {
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[
                ("meshes/", b""),
                ("meshes/armor.nif", b"nif_data"),
                ("fomod/", b""),
                ("fomod/ModuleConfig.xml", b"<config/>"),
            ],
        );
        let dest = tmp.join("mod_data");
        std::fs::create_dir_all(&dest).unwrap();

        install_fomod_files(
            &archive,
            &dest,
            &[FomodFile {
                source: "meshes/".to_string(),
                destination: "Data/meshes".to_string(),
                priority: 0,
            }],
        )
        .unwrap();

        assert!(dest.join("meshes").join("armor.nif").exists());
    }

    #[test]
    fn install_fomod_files_data_root_fallback_skips_fomod_config_dir() {
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[
                ("fomod/", b""),
                ("fomod/ModuleConfig.xml", b"<config/>"),
                ("plugin.esp", b"esp_data"),
            ],
        );
        let dest = tmp.join("mod_data");
        std::fs::create_dir_all(&dest).unwrap();

        install_fomod_files(
            &archive,
            &dest,
            &[FomodFile {
                source: "Data".to_string(),
                destination: "Data".to_string(),
                priority: 0,
            }],
        )
        .unwrap();

        assert!(dest.join("plugin.esp").exists());
        assert!(!dest.join("fomod").exists());
    }

    #[test]
    fn install_fomod_files_skips_empty_directory_entries() {
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[
                ("textures/", b""),
                ("textures/armor.dds", b"dds_data"),
                ("textures/empty/", b""),
                ("fomod/", b""),
                ("fomod/ModuleConfig.xml", b"<config/>"),
            ],
        );
        let dest = tmp.join("mod_data");
        std::fs::create_dir_all(&dest).unwrap();

        install_fomod_files(
            &archive,
            &dest,
            &[FomodFile {
                source: "textures".to_string(),
                destination: "Data/textures".to_string(),
                priority: 0,
            }],
        )
        .unwrap();

        assert!(dest.join("textures").join("armor.dds").exists());
        assert!(!dest.join("textures").join("empty").exists());
    }

    #[test]
    fn install_fomod_files_matches_sources_in_wrapped_archives() {
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[
                ("Aela Replacer/", b""),
                ("Aela Replacer/00 main/", b""),
                ("Aela Replacer/00 main/AelaStandalone.esp", b"esp_data"),
                (
                    "Aela Replacer/00 main/textures/Actors/Character/Aela/Head/femalehead.dds",
                    b"dds_data",
                ),
                ("Aela Replacer/fomod/", b""),
                ("Aela Replacer/fomod/ModuleConfig.xml", b"<config/>"),
            ],
        );
        let dest = tmp.join("mod_data");
        std::fs::create_dir_all(&dest).unwrap();

        install_fomod_files(
            &archive,
            &dest,
            &[FomodFile {
                source: "00 main".to_string(),
                destination: "Data".to_string(),
                priority: 0,
            }],
        )
        .unwrap();

        assert!(dest.join("AelaStandalone.esp").exists());
        assert!(
            dest.join("textures")
                .join("Actors")
                .join("Character")
                .join("Aela")
                .join("Head")
                .join("femalehead.dds")
                .exists()
        );
    }

    #[test]
    fn parse_fomod_xml_parses_plugin_dependencies_flags_and_image() {
        let xml = br#"
            <config>
              <moduleName>Example Mod</moduleName>
              <installSteps>
                <installStep name="Variants">
                  <optionalFileGroups>
                    <group name="Main" type="SelectAny">
                      <plugins>
                        <plugin name="Plus Variant">
                          <description>Use plus variant</description>
                          <image path="images/plus.png"/>
                          <conditionFlags>
                            <flag name="VariantSign">+</flag>
                          </conditionFlags>
                          <dependencies operator="And">
                            <flagDependency flag="FeaturePack" value="+"/>
                          </dependencies>
                          <files>
                            <file source="plus/file.txt" destination="Data/file.txt"/>
                          </files>
                        </plugin>
                      </plugins>
                    </group>
                  </optionalFileGroups>
                </installStep>
              </installSteps>
            </config>
        "#;

        let cfg = parse_fomod_xml(xml).unwrap();
        let plugin = &cfg.steps[0].groups[0].plugins[0];
        assert_eq!(plugin.image_path.as_deref(), Some("images/plus.png"));
        assert_eq!(
            plugin.condition_flags,
            vec![ConditionFlag {
                name: "VariantSign".to_string(),
                value: "+".to_string(),
            }]
        );
        assert_eq!(
            plugin.dependencies,
            Some(PluginDependencies {
                operator: DependencyOperator::And,
                flags: vec![FlagDependency {
                    flag: "FeaturePack".to_string(),
                    value: "+".to_string(),
                }],
            })
        );
        assert!(cfg.steps[0].visible.is_none());
        assert!(cfg.conditional_file_installs.is_empty());
    }

    #[test]
    fn parse_fomod_xml_parses_step_visibility_and_conditional_files() {
        let xml = br#"
            <config>
              <moduleName>Example Mod</moduleName>
              <installSteps>
                <installStep name="Underwear Options">
                  <visible>
                    <flagDependency flag="bUnderwear" value="On" />
                  </visible>
                  <optionalFileGroups>
                    <group name="Color" type="SelectExactlyOne">
                      <plugins>
                        <plugin name="Black">
                          <files>
                            <folder source="16 Underwear" destination="" priority="0"/>
                          </files>
                        </plugin>
                      </plugins>
                    </group>
                  </optionalFileGroups>
                </installStep>
              </installSteps>
              <conditionalFileInstalls>
                <patterns>
                  <pattern>
                    <dependencies>
                      <flagDependency flag="bUnderwear" value="On"/>
                    </dependencies>
                    <files>
                      <folder source="22 Underwear Dark Purple" destination=""/>
                    </files>
                  </pattern>
                </patterns>
              </conditionalFileInstalls>
            </config>
        "#;

        let cfg = parse_fomod_xml(xml).unwrap();
        assert_eq!(cfg.steps.len(), 1);
        assert_eq!(
            cfg.steps[0].visible,
            Some(PluginDependencies {
                operator: DependencyOperator::And,
                flags: vec![FlagDependency {
                    flag: "bUnderwear".to_string(),
                    value: "On".to_string(),
                }],
            })
        );
        assert_eq!(cfg.conditional_file_installs.len(), 1);
        assert_eq!(
            cfg.conditional_file_installs[0].dependencies,
            PluginDependencies {
                operator: DependencyOperator::And,
                flags: vec![FlagDependency {
                    flag: "bUnderwear".to_string(),
                    value: "On".to_string(),
                }],
            }
        );
        assert_eq!(cfg.conditional_file_installs[0].files.len(), 1);
        assert_eq!(cfg.conditional_file_installs[0].files[0].source, "22 Underwear Dark Purple");
    }

    #[test]
    fn read_archive_file_bytes_finds_fomod_relative_image_path() {
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[
                ("MyMod/", b""),
                ("MyMod/fomod/", b""),
                ("MyMod/fomod/images/preview.png", b"pngdata"),
            ],
        );
        let bytes = read_archive_file_bytes(&archive, "images/preview.png").unwrap();
        assert_eq!(bytes, b"pngdata");
    }

    #[test]
    fn read_archive_file_bytes_non_zip_finds_image() {
        let tmp = tempdir();
        let Some(archive) = create_test_7z_with_image(&tmp, "mod_img.7z") else {
            return; // 7z not available
        };
        let bytes = read_archive_file_bytes(&archive, "images/preview.png").unwrap();
        assert_eq!(bytes, b"pngdata");
    }

    #[test]
    fn install_fomod_files_non_zip_installs_selected_files() {
        let tmp = tempdir();
        let Some(archive) = create_test_7z(&tmp, "mod_fomod.7z") else {
            return; // 7z not available
        };
        let dest = tmp.join("mod_data");
        std::fs::create_dir_all(&dest).unwrap();

        // The test 7z has Data/textures/sky.dds; ask for that via a FOMOD
        // file entry with source="textures" destination="Data/textures".
        install_fomod_files(
            &archive,
            &dest,
            &[FomodFile {
                source: "textures".to_string(),
                destination: "Data/textures".to_string(),
                priority: 0,
            }],
        )
        .unwrap();

        assert!(dest.join("textures").join("sky.dds").exists());
        assert_eq!(
            std::fs::read(dest.join("textures").join("sky.dds")).unwrap(),
            b"dds",
        );
    }

    #[test]
    fn install_fomod_files_from_dir_same_result_as_zip() {
        // Verify that install_fomod_files_from_dir produces the same layout as
        // the zip-based code by comparing results for identical content.
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[
                ("fomod/", b""),
                ("fomod/ModuleConfig.xml", b"<config/>"),
                ("textures/sky.dds", b"dds_data"),
            ],
        );
        let dest_zip = tmp.join("dest_zip");
        std::fs::create_dir_all(&dest_zip).unwrap();
        let files = vec![FomodFile {
            source: "textures".to_string(),
            destination: "Data/textures".to_string(),
            priority: 0,
        }];
        install_fomod_files(&archive, &dest_zip, &files).unwrap();

        // Now do the same with the dir-based function using a matching tree.
        let extracted = tmp.join("extracted");
        std::fs::create_dir_all(extracted.join("fomod")).unwrap();
        std::fs::write(extracted.join("fomod/ModuleConfig.xml"), b"<config/>").unwrap();
        std::fs::create_dir_all(extracted.join("textures")).unwrap();
        std::fs::write(extracted.join("textures/sky.dds"), b"dds_data").unwrap();

        let dest_dir = tmp.join("dest_dir");
        std::fs::create_dir_all(&dest_dir).unwrap();
        install_fomod_files_from_dir(&extracted, &dest_dir, &files).unwrap();

        assert!(dest_dir.join("textures").join("sky.dds").exists());
        assert_eq!(
            std::fs::read(dest_zip.join("textures").join("sky.dds")).unwrap(),
            std::fs::read(dest_dir.join("textures").join("sky.dds")).unwrap(),
        );
    }

    fn tempdir() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static CTR: AtomicU32 = AtomicU32::new(0);
        let n = CTR.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("linkmm_test_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
