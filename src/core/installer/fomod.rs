use std::collections::HashSet;
use std::io::Read;
use std::path::Path;

use super::archive::open_7z_reader;
use super::extract::{ExtractedArchive, create_temp_extract_dir, extract_single_rar_file};
use super::paths::{
    has_rar_extension, has_zip_extension, installer_log_warning, normalize_path_lowercase,
};
use super::types::*;

/// Parse a FOMOD `ModuleConfig.xml` from a supported archive.
#[allow(dead_code)]
pub fn parse_fomod_from_archive(archive_path: &Path) -> Result<FomodConfig, String> {
    let archive_name = archive_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let _span = crate::core::logger::span("parse_fomod", &format!("archive={archive_name}"));

    let mut config = if has_zip_extension(archive_path) {
        parse_fomod_from_zip(archive_path)?
    } else if has_rar_extension(archive_path) {
        parse_fomod_from_rar(archive_path)?
    } else {
        let mut reader = open_7z_reader(archive_path)?;

        let fomod_entry = reader
            .archive()
            .files
            .iter()
            .find(|f| {
                let lower = f.name().to_lowercase().replace('\\', "/");
                lower == "fomod/moduleconfig.xml" || lower.ends_with("/fomod/moduleconfig.xml")
            })
            .map(|f| f.name().to_string())
            .ok_or_else(|| "No fomod/ModuleConfig.xml found in archive".to_string())?;

        log::debug!(
            "[FOMOD] Found config entry in 7z | exact_name={}",
            fomod_entry
        );

        let xml_bytes = reader.read_file(&fomod_entry).map_err(|e| {
            format!(
                "Failed to read '{}' from {}: {e}",
                fomod_entry,
                archive_path.display()
            )
        })?;

        parse_fomod_xml(&xml_bytes)?
    };

    if config.mod_name.is_none() && !archive_name.is_empty() {
        let stem = std::path::Path::new(&archive_name)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or(archive_name);
        config.mod_name = Some(stem);
    }

    Ok(config)
}

/// Parse a FOMOD `ModuleConfig.xml` from inside a RAR archive.
#[allow(dead_code)]
fn parse_fomod_from_rar(archive_path: &Path) -> Result<FomodConfig, String> {
    let entries = super::archive::list_rar_entries(archive_path)?;
    let fomod_entry = entries
        .iter()
        .find(|p| {
            let lower = p.to_lowercase().replace('\\', "/");
            lower == "fomod/moduleconfig.xml" || lower.ends_with("/fomod/moduleconfig.xml")
        })
        .ok_or_else(|| "No fomod/ModuleConfig.xml found in archive".to_string())?;

    let tmp_extract = create_temp_extract_dir()?;
    let result = (|| {
        extract_single_rar_file(archive_path, fomod_entry, &tmp_extract)?;
        let config_path = find_fomod_config_in_dir(&tmp_extract)
            .ok_or_else(|| "fomod/ModuleConfig.xml not found after extraction".to_string())?;
        let xml_bytes = std::fs::read(&config_path)
            .map_err(|e| format!("Failed to read fomod config {}: {e}", config_path.display()))?;
        parse_fomod_xml(&xml_bytes)
    })();

    if let Err(e) = std::fs::remove_dir_all(&tmp_extract) {
        installer_log_warning(format!(
            "Failed to remove temporary extraction directory {}: {e}",
            tmp_extract.display()
        ));
    }

    result
}

/// Parse a FOMOD `ModuleConfig.xml` from inside a zip archive.
#[allow(dead_code)]
pub fn parse_fomod_from_zip(archive_path: &Path) -> Result<FomodConfig, String> {
    let file =
        std::fs::File::open(archive_path).map_err(|e| format!("Cannot open archive: {e}"))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| format!("Cannot read zip archive: {e}"))?;

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

/// Parse a FOMOD `ModuleConfig.xml` from an already-extracted archive.
///
/// Works entirely from the filesystem — no archive re-reading required.
pub fn parse_fomod_from_extracted(
    extracted: &ExtractedArchive,
    archive_name: &str,
) -> Result<FomodConfig, String> {
    let _span =
        crate::core::logger::span("parse_fomod_extracted", &format!("archive={archive_name}"));

    let config_entry = extracted
        .entries()
        .iter()
        .find(|e| {
            let lower = e.to_lowercase().replace('\\', "/");
            lower == "fomod/moduleconfig.xml" || lower.ends_with("/fomod/moduleconfig.xml")
        })
        .ok_or_else(|| "No fomod/ModuleConfig.xml found in extracted archive".to_string())?;

    let config_path = extracted.dir().join(config_entry.as_str());
    log::debug!(
        "[FOMOD] Reading config from extracted dir | path={}",
        config_path.display()
    );

    let xml_bytes = std::fs::read(&config_path)
        .map_err(|e| format!("Failed to read FOMOD config {}: {e}", config_path.display()))?;

    let mut config = parse_fomod_xml(&xml_bytes)?;

    if config.mod_name.is_none() && !archive_name.is_empty() {
        let stem = std::path::Path::new(archive_name)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| archive_name.to_string());
        config.mod_name = Some(stem);
    }

    Ok(config)
}

#[allow(dead_code)]
fn find_fomod_config_in_dir(root: &Path) -> Option<std::path::PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) => {
                installer_log_warning(format!(
                    "Failed to read extracted archive directory {}: {e}",
                    dir.display()
                ));
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

#[allow(dead_code)]
fn find_fomod_entry(zip: &mut zip::ZipArchive<std::fs::File>) -> Result<String, String> {
    for i in 0..zip.len() {
        let entry = zip
            .by_index(i)
            .map_err(|e| format!("Cannot read zip entry: {e}"))?;
        let lower = entry.name().to_lowercase().replace('\\', "/");
        if lower == "fomod/moduleconfig.xml" || lower.ends_with("/fomod/moduleconfig.xml") {
            return Ok(entry.name().to_string());
        }
    }
    Err("No fomod/ModuleConfig.xml found in archive".to_string())
}

/// Decode raw `ModuleConfig.xml` bytes to a UTF-8 `String`.
pub(super) fn decode_fomod_xml(raw: &[u8]) -> Result<String, String> {
    if raw.starts_with(&[0xFF, 0xFE]) {
        let payload = &raw[2..];
        if !payload.len().is_multiple_of(2) {
            return Err(format!(
                "UTF-16 LE data has odd byte count ({} bytes after BOM)",
                payload.len()
            ));
        }
        let utf16: Vec<u16> = payload
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        return String::from_utf16(&utf16).map_err(|e| format!("UTF-16 LE decode error: {e}"));
    }
    if raw.starts_with(&[0xFE, 0xFF]) {
        let payload = &raw[2..];
        if !payload.len().is_multiple_of(2) {
            return Err(format!(
                "UTF-16 BE data has odd byte count ({} bytes after BOM)",
                payload.len()
            ));
        }
        let utf16: Vec<u16> = payload
            .chunks_exact(2)
            .map(|b| u16::from_be_bytes([b[0], b[1]]))
            .collect();
        return String::from_utf16(&utf16).map_err(|e| format!("UTF-16 BE decode error: {e}"));
    }
    let bytes = raw.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(raw);
    match String::from_utf8(bytes.to_vec()) {
        Ok(s) => Ok(s),
        Err(_) => {
            // Many older FOMOD configs (especially from Windows tools) use
            // Windows-1252 or ISO-8859-1 encoding without a BOM.  Fall back
            // to Windows-1252 decoding so these archives can still install.
            log::info!(
                "[FOMOD] UTF-8 decode failed, falling back to Windows-1252 decoding ({} bytes)",
                bytes.len()
            );
            Ok(decode_windows_1252(bytes))
        }
    }
}

/// Decode a byte slice as Windows-1252 (superset of ISO-8859-1).
///
/// Windows-1252 is the most common legacy encoding for FOMOD XML files
/// created by older Windows modding tools.  Bytes 0x00–0x7F and 0xA0–0xFF
/// map to the same Unicode code points; bytes 0x80–0x9F map to various
/// typographic characters.
fn decode_windows_1252(bytes: &[u8]) -> String {
    /// Mapping for the 0x80–0x9F range in Windows-1252.
    /// Entries marked `\0` are undefined in the spec and mapped to the
    /// Unicode replacement character by the caller.
    const WIN1252_80_9F: [char; 32] = [
        '\u{20AC}', '\0', '\u{201A}', '\u{0192}', // 80–83
        '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}', // 84–87
        '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', // 88–8B
        '\u{0152}', '\0', '\u{017D}', '\0', // 8C–8F
        '\0', '\u{2018}', '\u{2019}', '\u{201C}', // 90–93
        '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}', // 94–97
        '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', // 98–9B
        '\u{0153}', '\0', '\u{017E}', '\u{0178}', // 9C–9F
    ];

    bytes
        .iter()
        .map(|&b| {
            if !(0x80..0xA0).contains(&b) {
                b as char
            } else {
                let ch = WIN1252_80_9F[(b - 0x80) as usize];
                if ch == '\0' { '\u{FFFD}' } else { ch }
            }
        })
        .collect()
}

pub(super) fn parse_fomod_xml(xml_bytes: &[u8]) -> Result<FomodConfig, String> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let xml_str = decode_fomod_xml(xml_bytes)?;
    let mut reader = Reader::from_str(&xml_str);
    reader.config_mut().trim_text(true);

    let mut config = FomodConfig {
        mod_name: None,
        required_files: Vec::new(),
        steps: Vec::new(),
        conditional_file_installs: Vec::new(),
    };

    let mut buf = Vec::new();
    let mut path_stack: Vec<String> = Vec::new();

    let mut current_step: Option<FomodInstallStep> = None;
    let mut current_group: Option<FomodPluginGroup> = None;
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
                        current_step = Some(FomodInstallStep {
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
                            "selectatleastone" => FomodGroupType::SelectAtLeastOne,
                            "selectatmostone" => FomodGroupType::SelectAtMostOne,
                            "selectexactlyone" => FomodGroupType::SelectExactlyOne,
                            "selectall" => FomodGroupType::SelectAll,
                            _ => FomodGroupType::SelectAny,
                        };
                        current_group = Some(FomodPluginGroup {
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
                            type_descriptor: FomodPluginType::Optional,
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
                                    "required" => FomodPluginType::Required,
                                    "recommended" => FomodPluginType::Recommended,
                                    "notusable" | "couldbeusable" => FomodPluginType::NotUsable,
                                    _ => FomodPluginType::Optional,
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
                            } else if let Some(ref mut plugin) = current_plugin
                                && let Some(ref mut deps) = plugin.dependencies
                            {
                                deps.flags.push(FlagDependency { flag, value });
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
                                "required" => FomodPluginType::Required,
                                "recommended" => FomodPluginType::Recommended,
                                "notusable" | "couldbeusable" => FomodPluginType::NotUsable,
                                _ => FomodPluginType::Optional,
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
                        if in_condition_flags && let Some(ref mut plugin) = current_plugin {
                            let name = get_attr(e, "name").unwrap_or_default();
                            let value = get_attr(e, "value").unwrap_or_default();
                            if !name.is_empty() && !value.is_empty() {
                                plugin.condition_flags.push(ConditionFlag { name, value });
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
                            } else if let Some(ref mut plugin) = current_plugin
                                && let Some(ref mut deps) = plugin.dependencies
                            {
                                deps.flags.push(FlagDependency { flag, value });
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
                        if let Some(ref mut plugin) = current_plugin
                            && !current_text.is_empty()
                        {
                            plugin.description = Some(current_text.clone());
                        }
                    }
                    "flag" => {
                        let in_condition_flags = path_stack
                            .last()
                            .map(|p| p == "conditionflags")
                            .unwrap_or(false);
                        if in_condition_flags
                            && let Some(name) = current_condition_flag_name.take()
                            && let Some(ref mut plugin) = current_plugin
                            && !current_text.is_empty()
                        {
                            plugin.condition_flags.push(ConditionFlag {
                                name,
                                value: current_text.clone(),
                            });
                        }
                    }
                    "dependencies" => {
                        if in_pattern {
                            if let Some(ref deps) = current_pattern_dependencies
                                && deps.flags.is_empty()
                            {
                                current_pattern_dependencies = None;
                            }
                        } else if in_visible {
                            if let Some(ref mut step) = current_step
                                && let Some(ref deps) = step.visible
                                && deps.flags.is_empty()
                            {
                                step.visible = None;
                            }
                        } else if let Some(ref mut plugin) = current_plugin
                            && let Some(ref deps) = plugin.dependencies
                            && deps.flags.is_empty()
                        {
                            plugin.dependencies = None;
                        }
                    }
                    "visible" => {
                        in_visible = false;
                    }
                    "plugin" => {
                        if let Some(plugin) = current_plugin.take()
                            && let Some(ref mut group) = current_group
                        {
                            group.plugins.push(plugin);
                        }
                    }
                    "group" => {
                        if let Some(group) = current_group.take()
                            && let Some(ref mut step) = current_step
                        {
                            step.groups.push(group);
                        }
                    }
                    "installstep" => {
                        if let Some(step) = current_step.take() {
                            config.steps.push(step);
                        }
                    }
                    "pattern" => {
                        let dependencies =
                            current_pattern_dependencies
                                .take()
                                .unwrap_or(PluginDependencies {
                                    operator: DependencyOperator::And,
                                    flags: Vec::new(),
                                });
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

    log::debug!(
        "[FOMOD] XML parsed | mod_name={}, steps={}, required_files={}, conditional_patterns={}",
        config.mod_name.as_deref().unwrap_or("<unnamed>"),
        config.steps.len(),
        config.required_files.len(),
        config.conditional_file_installs.len()
    );

    if config.steps.is_empty() && config.required_files.is_empty() {
        log::warn!(
            "[FOMOD] Parsed config has no steps and no required files. \
             Check if the XML was decoded correctly (possible encoding issue)."
        );
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

/// Resolve conflicts within a file installation list.
///
/// When multiple files target the same destination:
/// 1. Higher priority wins
/// 2. Later position in list wins (tie-breaker)
/// 3. Keep only the winner for each destination
#[allow(dead_code)]
pub fn resolve_file_conflicts(mut files: Vec<FomodFile>) -> Vec<FomodFile> {
    if files.is_empty() {
        return files;
    }

    // Assign positional indices for stable tie-breaking
    let mut indexed: Vec<(usize, FomodFile)> = files.drain(..).enumerate().collect();

    // Sort: destination (asc), priority (desc), position (desc)
    indexed.sort_by(|(idx_a, a), (idx_b, b)| {
        normalize_path_lowercase(&a.destination)
            .cmp(&normalize_path_lowercase(&b.destination))
            .then(b.priority.cmp(&a.priority))
            .then(idx_b.cmp(idx_a))
    });

    // Keep first occurrence of each destination (= winner)
    let mut seen = HashSet::new();
    let total_before = indexed.len();
    indexed.retain(|(_, f)| {
        let dest_norm = normalize_path_lowercase(&f.destination);
        let is_new = seen.insert(dest_norm.clone());
        if !is_new {
            log::debug!(
                "[Conflict] Duplicate destination skipped | dest={}, source={}, priority={}",
                dest_norm,
                f.source,
                f.priority
            );
        }
        is_new
    });
    let conflicts_resolved = total_before - indexed.len();
    if conflicts_resolved > 0 {
        log::info!(
            "[Conflict] Resolved {} file conflicts, {} files remaining",
            conflicts_resolved,
            indexed.len()
        );
    }

    indexed.into_iter().map(|(_, f)| f).collect()
}
