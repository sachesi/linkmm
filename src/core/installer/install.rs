use std::io::BufWriter;
use std::path::{Path, PathBuf};

use crate::core::games::Game;
use crate::core::logger;
use crate::core::mods::{Mod, ModDatabase, ModManager};

use super::archive::list_archive_entries_with_7z;
use super::extract::{
    ExtractedArchive, create_temp_extract_dir, extract_archive_with_7z, extract_non_zip_to,
    extract_zip_to, normalize_paths_to_lowercase,
};
use super::heuristics::{build_data_archive_plan, find_data_root_in_paths};
use super::paths::{
    has_rar_extension, has_zip_extension, installer_log_activity, installer_log_warning,
    is_safe_relative_path, normalize_path, strip_data_prefix,
};
use super::types::*;

/// Recursively collect all regular files under `dir`.
#[allow(dead_code)]
fn collect_fs_files(root: &Path, dir: &Path, result: &mut Vec<(String, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_fs_files(root, &path, result);
        } else if path.is_file()
            && let Ok(rel) = path.strip_prefix(root)
        {
            let rel_str = normalize_path(&rel.to_string_lossy());
            result.push((rel_str.to_lowercase(), path));
        }
    }
}

/// Recursively collect all entries (files and directories) under `dir`.
fn collect_fs_entries(root: &Path, dir: &Path, result: &mut Vec<(String, String)>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let rel_str = normalize_path(&rel.to_string_lossy());
        if rel_str.is_empty() {
            continue;
        }
        result.push((rel_str.to_lowercase(), rel_str));
        if path.is_dir() {
            collect_fs_entries(root, &path, result);
        }
    }
}

/// Filter `entry_map` to entries matching `source_lower`.
fn collect_matching_fs_entries(entry_map: &[(String, String)], source_lower: &str) -> Vec<String> {
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

// ── Installation logic ────────────────────────────────────────────────────────

/// Install a mod from an archive.
#[allow(dead_code)]
pub fn install_mod_from_archive(
    archive_path: &Path,
    game: &Game,
    mod_name: &str,
    strategy: &InstallStrategy,
) -> Result<Mod, String> {
    install_mod_from_archive_with_nexus_ticking(
        archive_path,
        game,
        mod_name,
        strategy,
        None,
        &|| {},
    )
}

pub fn install_mod_from_archive_with_nexus(
    archive_path: &Path,
    game: &Game,
    mod_name: &str,
    strategy: &InstallStrategy,
    nexus_id: Option<u32>,
) -> Result<Mod, String> {
    install_mod_from_archive_with_nexus_ticking(
        archive_path,
        game,
        mod_name,
        strategy,
        nexus_id,
        &|| {},
    )
}

pub fn install_mod_from_archive_with_nexus_ticking(
    archive_path: &Path,
    game: &Game,
    mod_name: &str,
    strategy: &InstallStrategy,
    nexus_id: Option<u32>,
    tick: &dyn Fn(),
) -> Result<Mod, String> {
    let archive_name = archive_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let _span = logger::span(
        "install_mod",
        &format!("archive={archive_name}, mod={mod_name}"),
    );

    let mod_dir = ModManager::create_mod_directory(game)?;

    match strategy {
        InstallStrategy::Data => {
            if !has_zip_extension(archive_path) {
                install_data_archive_non_zip(archive_path, &mod_dir, tick)?;
            } else {
                install_zip_data_mod(archive_path, &mod_dir, tick)?;
            }
        }
        InstallStrategy::Fomod(files) => {
            if files.is_empty() {
                let _ = std::fs::remove_dir_all(&mod_dir);
                return Err("No files selected for installation. FOMOD configuration may be invalid or no options were selected.".to_string());
            }

            let data_dir = mod_dir.join("Data");
            std::fs::create_dir_all(&data_dir)
                .map_err(|e| format!("Failed to create Data directory: {e}"))?;
            install_fomod(archive_path, &data_dir, files)?;

            let has_files = data_dir
                .read_dir()
                .ok()
                .and_then(|mut entries| entries.next())
                .is_some();
            if !has_files {
                let _ = std::fs::remove_dir_all(&mod_dir);
                return Err(
                    "No files were installed. FOMOD file paths may not match archive contents."
                        .to_string(),
                );
            }

            normalize_paths_to_lowercase(&data_dir);
        }
    }

    let mut mod_entry = Mod::new(mod_name, mod_dir);
    mod_entry.installed_from_nexus = nexus_id.is_some();
    mod_entry.nexus_id = nexus_id;
    mod_entry.archive_name = archive_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned());

    let mut db = ModDatabase::load(game);
    db.mods.retain(|m| m.name != mod_name);
    db.mods.push(mod_entry.clone());
    db.save(game);

    Ok(mod_entry)
}

/// Install a zip-format Data mod.
fn install_zip_data_mod(
    archive_path: &Path,
    mod_dir: &Path,
    tick: &dyn Fn(),
) -> Result<(), String> {
    let file =
        std::fs::File::open(archive_path).map_err(|e| format!("Cannot open archive: {e}"))?;
    let zip = zip::ZipArchive::new(file).map_err(|e| format!("Cannot read zip archive: {e}"))?;
    let all_paths: Vec<String> = zip.file_names().map(|s| s.to_string()).collect();
    drop(zip);
    let path_refs: Vec<&str> = all_paths.iter().map(|s| s.as_str()).collect();

    let data_dir = mod_dir.join("Data");
    std::fs::create_dir_all(&data_dir)
        .map_err(|e| format!("Failed to create Data directory: {e}"))?;

    let plan = build_data_archive_plan(&path_refs);
    installer_log_activity(format!(
        "Archive install plan for {}: {:?}",
        archive_path.display(),
        plan
    ));

    match plan {
        DataArchivePlan::Bain { top_dirs } => {
            for bain_dir in top_dirs {
                let bain_prefix = format!("{bain_dir}/");
                extract_zip_to(archive_path, &data_dir, &bain_prefix, &|_, _| {
                    tick();
                    true
                })?;
            }
            normalize_paths_to_lowercase(&data_dir);
        }
        DataArchivePlan::ExtractToData { strip_prefix } => {
            extract_zip_to(archive_path, &data_dir, &strip_prefix, &|_, _| {
                tick();
                true
            })?;
            normalize_paths_to_lowercase(&data_dir);
        }
        DataArchivePlan::ExtractToModRoot { strip_prefix } => {
            extract_zip_to(archive_path, mod_dir, &strip_prefix, &|_, _| {
                tick();
                true
            })?;
        }
    }

    Ok(())
}

fn install_data_archive_non_zip(
    archive_path: &Path,
    mod_dir: &Path,
    tick: &dyn Fn(),
) -> Result<(), String> {
    let entries = list_archive_entries_with_7z(archive_path).unwrap_or_default();
    let path_refs: Vec<&str> = entries.iter().map(|s| s.as_str()).collect();

    let data_dir = mod_dir.join("Data");
    std::fs::create_dir_all(&data_dir)
        .map_err(|e| format!("Failed to create Data directory: {e}"))?;

    let plan = build_data_archive_plan(&path_refs);
    installer_log_activity(format!(
        "Archive install plan for {}: {:?}",
        archive_path.display(),
        plan
    ));

    let install_result = match plan {
        DataArchivePlan::Bain { top_dirs } => {
            for bain_dir in top_dirs {
                let bain_prefix = format!("{bain_dir}/");
                extract_non_zip_to(archive_path, &data_dir, &bain_prefix, &|_, _| {
                    tick();
                    true
                })?;
            }
            Ok(())
        }
        DataArchivePlan::ExtractToData { strip_prefix } => {
            extract_non_zip_to(archive_path, &data_dir, &strip_prefix, &|_, _| {
                tick();
                true
            })
        }
        DataArchivePlan::ExtractToModRoot { strip_prefix } => {
            extract_non_zip_to(archive_path, mod_dir, &strip_prefix, &|_, _| {
                tick();
                true
            })
        }
    };

    if data_dir.is_dir() {
        normalize_paths_to_lowercase(&data_dir);
    }

    install_result
}

/// Install FOMOD-selected files from an archive.
pub(super) fn install_fomod(
    archive_path: &Path,
    dest_dir: &Path,
    files: &[FomodFile],
) -> Result<(), String> {
    let archive_name = archive_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let _span = logger::span(
        "install_fomod",
        &format!("archive={archive_name}, file_mappings={}", files.len()),
    );

    let is_zip = has_zip_extension(archive_path);
    let mut zip_archive = if is_zip {
        let file =
            std::fs::File::open(archive_path).map_err(|e| format!("Cannot open archive: {e}"))?;
        Some(zip::ZipArchive::new(file).map_err(|e| format!("Cannot read zip archive: {e}"))?)
    } else {
        None
    };

    let entries = if let Some(zip) = &mut zip_archive {
        zip.file_names().map(|s| s.to_string()).collect::<Vec<_>>()
    } else {
        list_archive_entries_with_7z(archive_path)?
    };

    let archive_prefix = {
        let entry_refs: Vec<&str> = entries.iter().map(|s| s.as_str()).collect();
        // Use FOMOD-aware detection: finds the directory containing fomod/ModuleConfig.xml,
        // which is exactly what FOMOD source paths are relative to.
        // Falls back to find_common_prefix_from_paths for archives without a fomod/ dir.
        let root = find_data_root_in_paths(&entry_refs);
        normalize_path(&root)
    };
    let entry_map: Vec<(String, String, usize)> = entries
        .iter()
        .enumerate()
        .map(|(idx, e)| (normalize_path(e).to_lowercase(), e.clone(), idx))
        .collect();

    let mut sorted_files = files.to_vec();
    sorted_files.sort_by(|a, b| a.priority.cmp(&b.priority));

    let mut files_to_extract: Vec<(String, PathBuf, usize)> = Vec::new();

    for fomod_file in &sorted_files {
        let source = normalize_path(&fomod_file.source);
        let destination = strip_data_prefix(&normalize_path(&fomod_file.destination));
        let source_lower = source.to_lowercase();

        let (matched_source, matching_entries) = {
            let mut candidates = vec![source.clone()];
            let source_has_data = source_lower.starts_with("data/");

            if source_has_data {
                let stripped = strip_data_prefix(&source);
                if !stripped.is_empty() && stripped != source {
                    candidates.push(stripped);
                }
            } else if !source_lower.is_empty() && source_lower != "data" {
                candidates.push(format!("data/{}", source));
            }

            let mut final_candidates = Vec::new();
            for cand in &candidates {
                final_candidates.push(cand.clone());
                if !archive_prefix.is_empty() {
                    final_candidates.push(normalize_path(&format!(
                        "{}/{}",
                        archive_prefix.trim_end_matches('/'),
                        cand
                    )));
                }
            }

            let mut result_source = String::new();
            let mut result_entries = Vec::new();

            for cand in final_candidates {
                let entries = collect_matching_entries(&entry_map, &cand.to_lowercase());
                if !entries.is_empty() {
                    result_source = cand;
                    result_entries = entries;
                    break;
                }
            }

            if result_entries.is_empty() && (source_lower == "data" || source_lower == "data/") {
                result_entries = entry_map
                    .iter()
                    .filter(|(nl, _, _)| {
                        !(nl == "fomod" || nl.starts_with("fomod/") || nl.contains("/fomod/"))
                    })
                    .map(|(_, orig, idx)| (orig.clone(), *idx))
                    .collect();
                if !result_entries.is_empty() {
                    result_source = String::new();
                }
            }
            (result_source, result_entries)
        };

        if matching_entries.is_empty() {
            log::warn!(
                "[FOMOD] No archive entries matched source '{}' | archive_prefix='{}' | total_entries={}",
                fomod_file.source,
                archive_prefix,
                entry_map.len()
            );
            // Log a sample of actual entry keys so the mismatch can be diagnosed.
            for (nl, _, _) in entry_map.iter().take(10) {
                log::debug!("[FOMOD] entry_map sample: '{nl}'");
            }
        }

        let matched_source_lower = matched_source.to_lowercase();
        let source_prefix_lower =
            (!matched_source_lower.is_empty()).then(|| format!("{matched_source_lower}/"));

        for (orig_entry, idx) in matching_entries {
            let entry_norm = normalize_path(&orig_entry);
            let entry_lower = entry_norm.to_lowercase();
            let rel = if matched_source_lower.is_empty() {
                if destination.is_empty() {
                    entry_norm.clone()
                } else {
                    format!("{}/{}", destination, entry_norm)
                }
            } else if entry_lower == matched_source_lower {
                destination.clone()
            } else if let Some(suffix) = source_prefix_lower.as_deref().and_then(|prefix| {
                entry_norm
                    .get(prefix.len()..)
                    .filter(|_| entry_lower.starts_with(prefix))
            }) {
                if destination.is_empty() {
                    suffix.to_string()
                } else {
                    format!("{}/{}", destination, suffix)
                }
            } else {
                continue;
            };

            if rel.is_empty() || rel.ends_with('/') {
                continue;
            }
            if !is_safe_relative_path(&rel) {
                installer_log_warning(format!("Skipping fomod entry with unsafe path: {rel}"));
                continue;
            }

            let dest_path = dest_dir.join(&rel);
            files_to_extract.push((orig_entry, dest_path, idx));
        }
    }

    installer_log_activity(format!(
        "Extracting {} files from FOMOD archive",
        files_to_extract.len()
    ));
    if let Some(zip) = &mut zip_archive {
        for (raw_name, out_path, entry_idx) in files_to_extract {
            let rel_lower = normalize_path(&raw_name).to_lowercase();
            // Skip known macOS / metadata entries that should never be extracted.
            let top = rel_lower.split('/').next().unwrap_or("");
            if JUNK_TOPLEVEL_ENTRIES.contains(&top) {
                continue;
            }
            let mut entry = zip
                .by_index(entry_idx)
                .map_err(|e| format!("Cannot read entry at index {entry_idx}: {e}"))?;
            if entry.is_dir() {
                continue;
            }
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create parent dir: {e}"))?;
            }
            let out_file = std::fs::File::create(&out_path)
                .map_err(|e| format!("Failed to create file: {e}"))?;
            let mut buffered = BufWriter::with_capacity(EXTRACT_BUFFER_SIZE, out_file);
            std::io::copy(&mut entry, &mut buffered)
                .map_err(|e| format!("Failed to extract: {e}"))?;
        }
    } else {
        if has_rar_extension(archive_path) {
            let tmp = create_temp_extract_dir()?;
            extract_archive_with_7z(archive_path, &tmp)?;
            let result = install_fomod_files_from_dir(&tmp, dest_dir, files);
            let _ = std::fs::remove_dir_all(&tmp);
            return result;
        }

        sevenz_rust2::decompress_file_with_extract_fn(
            archive_path,
            dest_dir,
            |entry, reader, _default_dest| {
                let entry_name = entry.name();

                let destinations: Vec<PathBuf> = files_to_extract
                    .iter()
                    .filter(|(src, _, _)| src == entry_name)
                    .map(|(_, dest, _)| dest.clone())
                    .collect();

                if !destinations.is_empty() && !entry.is_directory() {
                    if destinations.len() == 1 {
                        let dest_path = &destinations[0];
                        if let Some(parent) = dest_path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        if let Ok(out_file) = std::fs::File::create(dest_path) {
                            let mut buffered =
                                BufWriter::with_capacity(EXTRACT_BUFFER_SIZE, out_file);
                            let _ = std::io::copy(reader, &mut buffered);
                        }
                    } else {
                        let mut data = Vec::with_capacity(entry.size() as usize);
                        if reader.read_to_end(&mut data).is_ok() {
                            for dest_path in destinations {
                                if let Some(parent) = dest_path.parent() {
                                    let _ = std::fs::create_dir_all(parent);
                                }
                                let _ = std::fs::write(&dest_path, &data);
                            }
                        }
                    }
                }
                Ok(true)
            },
        )
        .map_err(|e| {
            format!(
                "Failed to extract 7z archive {}: {e}",
                archive_path.display()
            )
        })?;
    }

    Ok(())
}

#[allow(dead_code)]
pub(super) fn install_fomod_files(
    archive_path: &Path,
    dest_dir: &Path,
    files: &[FomodFile],
) -> Result<(), String> {
    install_fomod(archive_path, dest_dir, files)
}

/// Install FOMOD-selected files from an already-extracted directory tree.
pub(super) fn install_fomod_files_from_dir(
    extracted_dir: &Path,
    dest_dir: &Path,
    files: &[FomodFile],
) -> Result<(), String> {
    let mut entry_map: Vec<(String, String)> = Vec::new();
    collect_fs_entries(extracted_dir, extracted_dir, &mut entry_map);

    // FOMOD-aware prefix detection: finds the directory containing
    // fomod/ModuleConfig.xml, correctly handling multi-level nesting
    // (e.g. outer-dir/inner-dir/fomod/...).  This matches the logic
    // used by the archive-based install_fomod().
    let entry_strs: Vec<&str> = entry_map.iter().map(|(_, orig)| orig.as_str()).collect();
    let root = find_data_root_in_paths(&entry_strs);
    let root = root.replace('\\', "/");
    let (archive_prefix, archive_prefix_lower) = if root.is_empty() {
        (String::new(), String::new())
    } else {
        let p = if root.ends_with('/') {
            root
        } else {
            format!("{root}/")
        };
        let pl = p.to_lowercase();
        (p, pl)
    };

    let mut sorted_files = files.to_vec();
    sorted_files.sort_by(|a, b| a.priority.cmp(&b.priority));

    for fomod_file in &sorted_files {
        let source = normalize_path(&fomod_file.source);
        let destination = strip_data_prefix(&normalize_path(&fomod_file.destination));
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
                        !(nl == "fomod" || nl.starts_with("fomod/") || nl.contains("/fomod/"))
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

            if full_path.is_dir() {
                continue;
            }

            let entry_lower = orig_rel.to_lowercase();
            let rel = if entry_lower == matched_source_lower {
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

            let combined = if destination.is_empty() {
                std::borrow::Cow::Borrowed(rel.as_str())
            } else {
                std::borrow::Cow::Owned(format!("{destination}/{rel}"))
            };
            if !is_safe_relative_path(combined.as_ref()) {
                installer_log_warning(format!("Skipping fomod entry with unsafe path: {combined}"));
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

// ── Install from pre-extracted archive ────────────────────────────────────────

/// Install a mod from a pre-extracted archive.
///
/// This is the preferred path for all formats: the archive has already been
/// decompressed once by [`ExtractedArchive::from_archive`], so installation
/// is just filesystem copies — no re-reading of compressed data.
pub fn install_mod_from_extracted(
    extracted: &ExtractedArchive,
    game: &Game,
    mod_name: &str,
    strategy: &InstallStrategy,
    nexus_id: Option<u32>,
    archive_name: Option<&str>,
    tick: &dyn Fn(),
) -> Result<Mod, String> {
    let display_archive = archive_name.unwrap_or("(unknown)");
    let _span = logger::span(
        "install_mod_extracted",
        &format!("archive={display_archive}, mod={mod_name}"),
    );

    let mod_dir = ModManager::create_mod_directory(game)?;

    let result = match strategy {
        InstallStrategy::Data => install_data_from_extracted(extracted, &mod_dir, tick),
        InstallStrategy::Fomod(files) => {
            if files.is_empty() {
                let _ = std::fs::remove_dir_all(&mod_dir);
                return Err(
                    "No files selected for installation. FOMOD configuration may be \
                     invalid or no options were selected."
                        .to_string(),
                );
            }
            let data_dir = mod_dir.join("Data");
            std::fs::create_dir_all(&data_dir)
                .map_err(|e| format!("Failed to create Data directory: {e}"))?;

            install_fomod_files_from_dir(extracted.dir(), &data_dir, files)?;

            let has_files = data_dir
                .read_dir()
                .ok()
                .and_then(|mut entries| entries.next())
                .is_some();
            if !has_files {
                let _ = std::fs::remove_dir_all(&mod_dir);
                return Err("No files were installed. FOMOD file paths may not match \
                     archive contents."
                    .to_string());
            }

            normalize_paths_to_lowercase(&data_dir);
            Ok(())
        }
    };

    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&mod_dir);
        return Err(e);
    }

    // Files have been moved into `mod_dir` — remove the temp extract dir
    // immediately so the hidden folder in the mods directory does not linger
    // until the last Arc<ExtractedArchive> reference (e.g. a still-open
    // FOMOD wizard) is finally dropped.
    extracted.cleanup();

    let mut mod_entry = Mod::new(mod_name, mod_dir);
    mod_entry.installed_from_nexus = nexus_id.is_some();
    mod_entry.nexus_id = nexus_id;
    mod_entry.archive_name = archive_name.map(|s| s.to_owned());

    let mut db = ModDatabase::load(game);
    db.mods.retain(|m| m.name != mod_name);
    db.mods.push(mod_entry.clone());
    db.save(game);

    Ok(mod_entry)
}

/// Install a data mod from a pre-extracted archive.
fn install_data_from_extracted(
    extracted: &ExtractedArchive,
    mod_dir: &Path,
    tick: &dyn Fn(),
) -> Result<(), String> {
    let entries = extracted.entries();
    let path_refs: Vec<&str> = entries.iter().map(|s| s.as_str()).collect();

    let data_dir = mod_dir.join("Data");
    std::fs::create_dir_all(&data_dir)
        .map_err(|e| format!("Failed to create Data directory: {e}"))?;

    let plan = build_data_archive_plan(&path_refs);
    installer_log_activity(format!("Extracted archive install plan: {:?}", plan));

    match plan {
        DataArchivePlan::Bain { top_dirs } => {
            for bain_dir in top_dirs {
                let bain_prefix = format!("{bain_dir}/");
                move_from_extracted_to(extracted, &data_dir, &bain_prefix, tick)?;
            }
        }
        DataArchivePlan::ExtractToData { strip_prefix } => {
            move_from_extracted_to(extracted, &data_dir, &strip_prefix, tick)?;
        }
        DataArchivePlan::ExtractToModRoot { strip_prefix } => {
            move_from_extracted_to(extracted, mod_dir, &strip_prefix, tick)?;
        }
    }

    if data_dir.is_dir() {
        normalize_paths_to_lowercase(&data_dir);
    }

    Ok(())
}

/// Move files from a pre-extracted archive into `dest_dir`, stripping
/// `strip_prefix` from each entry path (case-insensitive prefix match).
///
/// Files are moved with [`std::fs::rename`] which is atomic and instant when
/// the source and destination are on the same filesystem (the common case when
/// the temp dir was created inside the game's mods directory via
/// [`ExtractedArchive::from_archive_in`]).  If `rename` fails — e.g. because
/// of a cross-device move — it falls back to copy + delete so the function
/// always succeeds regardless of filesystem layout.
fn move_from_extracted_to(
    extracted: &ExtractedArchive,
    dest_dir: &Path,
    strip_prefix: &str,
    tick: &dyn Fn(),
) -> Result<(), String> {
    let prefix_lower = strip_prefix.to_lowercase().replace('\\', "/");
    let mut last_tick = std::time::Instant::now();

    for entry in extracted.entries() {
        let now = std::time::Instant::now();
        if now.duration_since(last_tick).as_millis() as u64 >= EXTRACTION_TICK_INTERVAL_MS {
            tick();
            last_tick = now;
        }

        let entry_lower = entry.to_lowercase().replace('\\', "/");
        let rel = if prefix_lower.is_empty() {
            entry.as_str()
        } else if let Some(rest) = entry_lower.strip_prefix(&prefix_lower) {
            // Use the original-cased tail from `entry`.
            &entry[entry.len() - rest.len()..]
        } else {
            continue;
        };

        let rel = rel.trim_start_matches('/');
        if rel.is_empty() {
            continue;
        }
        if !is_safe_relative_path(rel) {
            installer_log_warning(format!("Skipping entry with unsafe path: {rel}"));
            continue;
        }

        let src = extracted.dir().join(entry.as_str());
        let dst = dest_dir.join(rel);

        if src.is_dir() {
            std::fs::create_dir_all(&dst)
                .map_err(|e| format!("Failed to create directory {}: {e}", dst.display()))?;
        } else if src.is_file() {
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create parent dir: {e}"))?;
            }
            // Fast path: rename is atomic and instant on the same filesystem.
            // Slow fallback: cross-device or other rename error → copy + delete.
            if std::fs::rename(&src, &dst).is_err() {
                std::fs::copy(&src, &dst).map_err(|e| {
                    format!("Failed to move {} → {}: {e}", src.display(), dst.display())
                })?;
                let _ = std::fs::remove_file(&src);
            }
        }
    }

    Ok(())
}
