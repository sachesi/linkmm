use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use crate::core::mods::Mod;

#[derive(Debug, Clone, Default)]
pub(super) struct ConflictState {
    pub(super) overwrites: bool,
    pub(super) overwritten: bool,
    pub(super) files: BTreeSet<String>,
    pub(super) conflict_mods_by_file: BTreeMap<String, BTreeSet<String>>,
}

pub(super) fn compute_conflict_states(
    mods: &[Mod],
    selected_id: Option<&str>,
) -> HashMap<String, ConflictState> {
    let global_states = compute_global_conflict_states(mods);

    if let Some(selected_id) = selected_id {
        let Some(selected_idx) = mods.iter().position(|m| m.id == selected_id) else {
            return global_states;
        };

        let selected_files = collect_mod_target_files(&mods[selected_idx]);
        if selected_files.is_empty() {
            return global_states;
        }

        let mut states: HashMap<String, ConflictState> = HashMap::new();
        for (idx, m) in mods.iter().enumerate() {
            if idx == selected_idx {
                continue;
            }
            let files = collect_mod_target_files(m);
            if files.is_empty() {
                continue;
            }

            let shared: BTreeSet<String> = selected_files.intersection(&files).cloned().collect();
            if shared.is_empty() {
                continue;
            }

            // With selection active: preserve green/red directionality by order.
            if idx > selected_idx {
                states.entry(m.id.clone()).or_default().overwrites = true;
                states
                    .entry(selected_id.to_string())
                    .or_default()
                    .overwritten = true;
            } else {
                states.entry(m.id.clone()).or_default().overwritten = true;
                states
                    .entry(selected_id.to_string())
                    .or_default()
                    .overwrites = true;
            }

            states
                .entry(m.id.clone())
                .or_default()
                .files
                .extend(shared.iter().cloned());
            {
                let entry = states.entry(m.id.clone()).or_default();
                for file in &shared {
                    entry
                        .conflict_mods_by_file
                        .entry(file.clone())
                        .or_default()
                        .insert(mods[selected_idx].name.clone());
                }
            }
            states
                .entry(selected_id.to_string())
                .or_default()
                .files
                .extend(shared.iter().cloned());
            {
                let entry = states.entry(selected_id.to_string()).or_default();
                for file in &shared {
                    entry
                        .conflict_mods_by_file
                        .entry(file.clone())
                        .or_default()
                        .insert(m.name.clone());
                }
            }
        }
        // If selected mod has no conflicts, keep the global blue conflict mode.
        if states.is_empty() {
            global_states
        } else {
            states
        }
    } else {
        global_states
    }
}

fn compute_global_conflict_states(mods: &[Mod]) -> HashMap<String, ConflictState> {
    let mut states: HashMap<String, ConflictState> = HashMap::new();
    let all_files: Vec<BTreeSet<String>> = mods.iter().map(collect_mod_target_files).collect();

    for i in 0..mods.len() {
        if all_files[i].is_empty() {
            continue;
        }
        for j in (i + 1)..mods.len() {
            if all_files[j].is_empty() {
                continue;
            }
            let shared: BTreeSet<String> =
                all_files[i].intersection(&all_files[j]).cloned().collect();
            if shared.is_empty() {
                continue;
            }

            states
                .entry(mods[i].id.clone())
                .or_default()
                .files
                .extend(shared.iter().cloned());
            {
                let entry = states.entry(mods[i].id.clone()).or_default();
                for file in &shared {
                    entry
                        .conflict_mods_by_file
                        .entry(file.clone())
                        .or_default()
                        .insert(mods[j].name.clone());
                }
            }
            states
                .entry(mods[j].id.clone())
                .or_default()
                .files
                .extend(shared.iter().cloned());
            {
                let entry = states.entry(mods[j].id.clone()).or_default();
                for file in &shared {
                    entry
                        .conflict_mods_by_file
                        .entry(file.clone())
                        .or_default()
                        .insert(mods[i].name.clone());
                }
            }
        }
    }

    states
}

fn collect_mod_target_files(mod_entry: &Mod) -> BTreeSet<String> {
    let mut files = BTreeSet::new();
    let root = &mod_entry.source_path;
    let data_dir = root.join("Data");

    if data_dir.is_dir() {
        collect_files_recursive(&data_dir, &data_dir, "data", &mut files);

        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.file_name().map(|n| n == "Data").unwrap_or(false) {
                    continue;
                }
                if path.is_dir() {
                    collect_files_recursive(&path, root, "root", &mut files);
                } else if path.is_file()
                    && let Ok(rel) = path.strip_prefix(root)
                {
                    files.insert(normalize_relative_path("root", rel));
                }
            }
        }
    } else {
        collect_files_recursive(root, root, "data", &mut files);
    }

    files
}

fn collect_files_recursive(base: &Path, root: &Path, prefix: &str, files: &mut BTreeSet<String>) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files_recursive(&path, root, prefix, files);
        } else if path.is_file()
            && let Ok(rel) = path.strip_prefix(root)
        {
            files.insert(normalize_relative_path(prefix, rel));
        }
    }
}

fn normalize_relative_path(prefix: &str, rel: &Path) -> String {
    let rel = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
        .to_lowercase();
    format!("{prefix}/{rel}")
}
