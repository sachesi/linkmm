use std::io::Read;
use std::path::Path;

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
    pub files: Vec<FomodFile>,
    pub type_descriptor: PluginType,
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
    pub groups: Vec<PluginGroup>,
}

/// Parsed FOMOD configuration.
#[derive(Debug, Clone)]
pub struct FomodConfig {
    pub mod_name: Option<String>,
    pub required_files: Vec<FomodFile>,
    pub steps: Vec<InstallStep>,
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
    let prefix = find_common_prefix(&mut zip);

    let Ok(file2) = std::fs::File::open(archive_path) else {
        return false;
    };
    let Ok(mut zip2) = zip::ZipArchive::new(file2) else {
        return false;
    };
    for i in 0..zip2.len() {
        let Ok(entry) = zip2.by_index(i) else {
            continue;
        };
        let name = entry.name();
        let rel = if !prefix.is_empty() {
            name.strip_prefix(&prefix).unwrap_or(name)
        } else {
            name
        };
        let top = top_level_component(&rel.to_lowercase());
        if top == "data" {
            return true;
        }
    }
    false
}

// ── FOMOD XML parser ──────────────────────────────────────────────────────────

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

fn parse_fomod_xml(xml_bytes: &[u8]) -> Result<FomodConfig, String> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let mut reader = Reader::from_reader(xml_bytes);
    reader.config_mut().trim_text(true);

    let mut config = FomodConfig {
        mod_name: None,
        required_files: Vec::new(),
        steps: Vec::new(),
    };

    let mut buf = Vec::new();
    let mut path_stack: Vec<String> = Vec::new();

    // Current parsing context
    let mut current_step: Option<InstallStep> = None;
    let mut current_group: Option<PluginGroup> = None;
    let mut current_plugin: Option<FomodPlugin> = None;
    let mut current_text = String::new();
    let mut in_required = false;

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
                            groups: Vec::new(),
                        });
                    }
                    "group" => {
                        let name = get_attr(e, "name").unwrap_or_default();
                        let type_str = get_attr(e, "type")
                            .unwrap_or_else(|| "SelectAny".to_string());
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
                            files: Vec::new(),
                            type_descriptor: PluginType::Optional,
                        });
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

                        if in_required && current_plugin.is_none() {
                            config.required_files.push(fomod_file);
                        } else if let Some(ref mut plugin) = current_plugin {
                            plugin.files.push(fomod_file);
                        }
                    }
                    "type" => {
                        if current_plugin.is_some() {
                            let name = get_attr(e, "name").unwrap_or_default();
                            if let Some(ref mut plugin) = current_plugin {
                                plugin.type_descriptor =
                                    match name.to_lowercase().as_str() {
                                        "required" => PluginType::Required,
                                        "recommended" => PluginType::Recommended,
                                        "notusable" | "couldbeusable" => PluginType::NotUsable,
                                        _ => PluginType::Optional,
                                    };
                            }
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

                        if in_required && current_plugin.is_none() {
                            config.required_files.push(fomod_file);
                        } else if let Some(ref mut plugin) = current_plugin {
                            plugin.files.push(fomod_file);
                        }
                    }
                    "type" => {
                        if let Some(ref mut plugin) = current_plugin {
                            let name = get_attr(e, "name").unwrap_or_default();
                            plugin.type_descriptor =
                                match name.to_lowercase().as_str() {
                                    "required" => PluginType::Required,
                                    "recommended" => PluginType::Recommended,
                                    "notusable" | "couldbeusable" => PluginType::NotUsable,
                                    _ => PluginType::Optional,
                                };
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) => {
                current_text = e
                    .unescape()
                    .unwrap_or_default()
                    .to_string();
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

fn get_attr(
    event: &quick_xml::events::BytesStart<'_>,
    name: &str,
) -> Option<String> {
    for attr in event.attributes().flatten() {
        if attr.key.as_ref() == name.as_bytes() {
            return Some(
                attr.unescape_value()
                    .unwrap_or_default()
                    .to_string(),
            );
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
    let mod_dir = ModManager::create_mod_directory(game)?;

    match strategy {
        InstallStrategy::Data => {
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
    mod_entry.installed_from_nexus = true;

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
    // folder wrapping everything)
    let prefix = find_common_prefix(&mut zip);

    let file2 =
        std::fs::File::open(archive_path).map_err(|e| format!("Cannot open archive: {e}"))?;
    let mut zip2 =
        zip::ZipArchive::new(file2).map_err(|e| format!("Cannot read zip archive: {e}"))?;

    for i in 0..zip2.len() {
        let mut entry = zip2
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
            let mut out_file = std::fs::File::create(&out_path)
                .map_err(|e| format!("Failed to create file {}: {e}", out_path.display()))?;
            std::io::copy(&mut entry, &mut out_file)
                .map_err(|e| format!("Failed to extract {}: {e}", rel_name))?;
        }
    }

    Ok(())
}

/// Detect whether all entries in the archive share a common top-level directory
/// prefix.  If so, return it (with trailing `/`).
fn find_common_prefix(zip: &mut zip::ZipArchive<std::fs::File>) -> String {
    if zip.is_empty() {
        return String::new();
    }

    let mut first_top: Option<String> = None;
    let mut all_same = true;

    for i in 0..zip.len() {
        let Ok(entry) = zip.by_index(i) else {
            continue;
        };
        let name = entry.name();
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

/// Install FOMOD-selected files from a zip archive.
///
/// For each `FomodFile`, extract the `source` path from the archive and place it
/// at `dest_dir / destination`.
fn install_fomod_files(
    archive_path: &Path,
    dest_dir: &Path,
    files: &[FomodFile],
) -> Result<(), String> {
    let file =
        std::fs::File::open(archive_path).map_err(|e| format!("Cannot open archive: {e}"))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| format!("Cannot read zip archive: {e}"))?;

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
        let destination = normalise_path(&fomod_file.destination);
        let source_lower = source.to_lowercase();

        // Find matching entry indices
        let matching_indices: Vec<(String, usize)> = entry_map
            .iter()
            .filter(|(nl, _, _)| {
                *nl == source_lower
                    || nl.starts_with(&format!("{source_lower}/"))
                    || nl.starts_with(&format!("{source_lower}\\"))
            })
            .map(|(_, orig, idx)| (orig.clone(), *idx))
            .collect();

        for (entry_name, entry_idx) in matching_indices {
            let entry_lower = entry_name.to_lowercase();
            // Compute the relative portion after the source prefix
            let rel = if entry_lower == source_lower {
                // Single file
                Path::new(&entry_name)
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_default()
            } else {
                entry_name[source.len()..].trim_start_matches('/').to_string()
            };

            if rel.is_empty() {
                continue;
            }

            // Zip-slip protection on the combined destination + rel path
            let combined = format!("{}/{}", destination, rel);
            if !is_safe_relative_path(&combined) {
                log::warn!("Skipping fomod entry with unsafe path: {combined}");
                continue;
            }

            let out_path = dest_dir.join(&destination).join(&rel);

            // Use by_index to avoid long-lived borrow issues
            let mut entry = zip
                .by_index(entry_idx)
                .map_err(|e| format!("Cannot read entry {entry_name}: {e}"))?;

            if entry.is_dir() {
                std::fs::create_dir_all(&out_path)
                    .map_err(|e| format!("Failed to create dir: {e}"))?;
            } else {
                if let Some(parent) = out_path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("Failed to create parent dir: {e}"))?;
                }

                let mut out_file = std::fs::File::create(&out_path)
                    .map_err(|e| format!("Failed to create file: {e}"))?;
                std::io::copy(&mut entry, &mut out_file)
                    .map_err(|e| format!("Failed to extract: {e}"))?;
            }
        }
    }

    Ok(())
}

/// Normalise path separators: backslash → forward slash, strip leading slash.
fn normalise_path(p: &str) -> String {
    let s = p.replace('\\', "/");
    s.strip_prefix('/').unwrap_or(&s).to_string()
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

    #[test]
    fn detect_strategy_data_for_loose_files() {
        let tmp = tempdir();
        let archive = create_test_zip(&tmp, &[
            ("textures/sky.dds", b"dds"),
            ("meshes/rock.nif", b"nif"),
        ]);
        let strategy = detect_strategy(&archive).unwrap();
        assert!(matches!(strategy, InstallStrategy::Data));
    }

    #[test]
    fn detect_strategy_root_for_data_folder() {
        let tmp = tempdir();
        let archive = create_test_zip(&tmp, &[
            ("Data/", b""),
            ("Data/textures/sky.dds", b"dds"),
            ("Data/meshes/rock.nif", b"nif"),
        ]);
        let strategy = detect_strategy(&archive).unwrap();
        assert!(matches!(strategy, InstallStrategy::Root));
    }

    #[test]
    fn extract_zip_data_strategy_puts_files_at_root_of_mod_dir() {
        let tmp = tempdir();
        let archive = create_test_zip(&tmp, &[
            ("textures/sky.dds", b"dds_data"),
            ("plugin.esp", b"esp_data"),
        ]);
        let dest = tmp.join("extracted");
        std::fs::create_dir_all(&dest).unwrap();
        extract_zip_to(&archive, &dest).unwrap();

        assert!(dest.join("textures").join("sky.dds").exists());
        assert!(dest.join("plugin.esp").exists());
        assert_eq!(std::fs::read_to_string(dest.join("plugin.esp")).unwrap(), "esp_data");
    }

    #[test]
    fn extract_zip_strips_common_prefix() {
        let tmp = tempdir();
        let archive = create_test_zip(&tmp, &[
            ("MyMod/", b""),
            ("MyMod/textures/sky.dds", b"dds_data"),
            ("MyMod/plugin.esp", b"esp_data"),
        ]);
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

    fn tempdir() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static CTR: AtomicU32 = AtomicU32::new(0);
        let n = CTR.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "linkmm_test_{}_{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
