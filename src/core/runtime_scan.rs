use crate::core::games::Game;
use crate::core::generated_outputs::adopt_existing_game_data_files;
use crate::core::mods::ModDatabase;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeEntryClassification {
    ManagedOwnedPresent,
    ManagedOwnedMissing,
    ManagedOwnedModified,
    UnmanagedAdoptable,
    UnmanagedIgnorable,
    UnknownNeedsReview,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeEntryReviewStatus {
    Pending,
    Ignored,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeScanEntry {
    pub relative_path: PathBuf,
    pub classification: RuntimeEntryClassification,
    pub review_status: RuntimeEntryReviewStatus,
    pub package_id: Option<String>,
    pub tool_id: Option<String>,
    pub explanation: String,
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeScanReport {
    pub entries: Vec<RuntimeScanEntry>,
}

impl RuntimeScanReport {
    pub fn unresolved_review_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| {
                e.review_status == RuntimeEntryReviewStatus::Pending
                    && matches!(
                        e.classification,
                        RuntimeEntryClassification::UnmanagedAdoptable
                            | RuntimeEntryClassification::UnknownNeedsReview
                            | RuntimeEntryClassification::ManagedOwnedMissing
                            | RuntimeEntryClassification::ManagedOwnedModified
                    )
            })
            .count()
    }

    pub fn adoptable_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| {
                e.review_status == RuntimeEntryReviewStatus::Pending
                    && e.classification == RuntimeEntryClassification::UnmanagedAdoptable
            })
            .count()
    }

    pub fn unknown_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| {
                e.review_status == RuntimeEntryReviewStatus::Pending
                    && e.classification == RuntimeEntryClassification::UnknownNeedsReview
            })
            .count()
    }

    pub fn has_unresolved_changes(&self) -> bool {
        self.unresolved_review_count() > 0
    }
}

#[derive(Debug, Clone, Default)]
struct OwnedFileRecord {
    package_id: String,
    tool_id: String,
    source_path: PathBuf,
}

pub fn scan_profile_runtime_changes(
    game: &Game,
    db: &ModDatabase,
) -> Result<RuntimeScanReport, String> {
    let active_profile = &db.active_profile_id;
    let ignored = db
        .profile_runtime_ignored
        .get(active_profile)
        .cloned()
        .unwrap_or_default();

    let mut owned_records = BTreeMap::<PathBuf, OwnedFileRecord>::new();
    for pkg in db
        .generated_outputs
        .iter()
        .filter(|p| p.manager_profile_id == *active_profile)
    {
        for owned in &pkg.owned_files {
            owned_records.insert(
                owned.relative_path.clone(),
                OwnedFileRecord {
                    package_id: pkg.id.clone(),
                    tool_id: pkg.tool_id.clone(),
                    source_path: pkg.source_path.join(&owned.relative_path),
                },
            );
        }
    }

    let data_files = collect_data_files(&game.data_path)?;

    let mut entries = Vec::new();

    for (rel, record) in &owned_records {
        let game_file = game.data_path.join(rel);
        if !data_files.contains(rel) {
            entries.push(RuntimeScanEntry {
                relative_path: rel.clone(),
                classification: RuntimeEntryClassification::ManagedOwnedMissing,
                review_status: review_status_for(rel, &ignored),
                package_id: Some(record.package_id.clone()),
                tool_id: Some(record.tool_id.clone()),
                explanation: "Managed/generated file is missing from Data".to_string(),
            });
            continue;
        }
        let modified = if game_file.is_symlink() {
            std::fs::read_link(&game_file)
                .map(|target| target != record.source_path)
                .unwrap_or(true)
        } else {
            true
        };
        let classification = if modified {
            RuntimeEntryClassification::ManagedOwnedModified
        } else {
            RuntimeEntryClassification::ManagedOwnedPresent
        };
        let explanation = if modified {
            "Managed/generated file exists but no longer points to expected package source"
        } else {
            "Managed/generated file is present and linked to package source"
        };
        entries.push(RuntimeScanEntry {
            relative_path: rel.clone(),
            classification,
            review_status: review_status_for(rel, &ignored),
            package_id: Some(record.package_id.clone()),
            tool_id: Some(record.tool_id.clone()),
            explanation: explanation.to_string(),
        });
    }

    for rel in &data_files {
        if owned_records.contains_key(rel) {
            continue;
        }
        let classification = classify_unmanaged(rel);
        let explanation = match classification {
            RuntimeEntryClassification::UnmanagedAdoptable => {
                "Unmanaged runtime file in scoped Data area; can be adopted into managed outputs"
            }
            RuntimeEntryClassification::UnmanagedIgnorable => {
                "Likely temporary/log/cache runtime file; can be ignored"
            }
            RuntimeEntryClassification::UnknownNeedsReview => {
                "Unmanaged runtime file in scoped Data area; needs manual review"
            }
            _ => "",
        };
        entries.push(RuntimeScanEntry {
            relative_path: rel.clone(),
            classification,
            review_status: review_status_for(rel, &ignored),
            package_id: None,
            tool_id: None,
            explanation: explanation.to_string(),
        });
    }

    entries.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(RuntimeScanReport { entries })
}

pub fn adopt_runtime_paths(
    game: &Game,
    db: &mut ModDatabase,
    relative_paths: &[PathBuf],
) -> Result<Option<String>, String> {
    let adoptable: Vec<PathBuf> = relative_paths
        .iter()
        .filter(|p| classify_unmanaged(p) == RuntimeEntryClassification::UnmanagedAdoptable)
        .cloned()
        .collect();
    if adoptable.is_empty() {
        return Ok(None);
    }
    let run_ctx = crate::core::generated_outputs::ToolRunContext {
        tool_id: "runtime-adopted".to_string(),
        run_profile: db.active_profile_id.clone(),
    };
    let package_name = format!("Runtime Adopted ({})", db.active_profile_id);
    let package_id = adopt_existing_game_data_files(game, db, &run_ctx, &adoptable, &package_name)?;
    clear_ignored_paths_for_active_profile(db, &adoptable);
    Ok(Some(package_id))
}

pub fn set_runtime_path_ignored(db: &mut ModDatabase, relative_path: &Path, ignored: bool) {
    let profile_id = db.active_profile_id.clone();
    let ignored_set = db.profile_runtime_ignored.entry(profile_id).or_default();
    let path_key = relative_path.to_string_lossy().to_string();
    if ignored {
        ignored_set.insert(path_key);
    } else {
        ignored_set.remove(&path_key);
    }
}

pub fn clear_ignored_paths_for_active_profile(db: &mut ModDatabase, paths: &[PathBuf]) {
    let profile_id = db.active_profile_id.clone();
    let Some(set) = db.profile_runtime_ignored.get_mut(&profile_id) else {
        return;
    };
    for p in paths {
        set.remove(&p.to_string_lossy().to_string());
    }
}

fn review_status_for(relative_path: &Path, ignored: &HashSet<String>) -> RuntimeEntryReviewStatus {
    if ignored.contains(&relative_path.to_string_lossy().to_string()) {
        RuntimeEntryReviewStatus::Ignored
    } else {
        RuntimeEntryReviewStatus::Pending
    }
}

fn collect_data_files(data_path: &Path) -> Result<HashSet<PathBuf>, String> {
    let mut files = HashSet::new();
    if !data_path.exists() {
        return Ok(files);
    }
    collect_recursive(data_path, Path::new(""), &mut files)?;
    Ok(files)
}

fn collect_recursive(base: &Path, rel: &Path, out: &mut HashSet<PathBuf>) -> Result<(), String> {
    for entry in
        std::fs::read_dir(base).map_err(|e| format!("Failed reading {}: {e}", base.display()))?
    {
        let entry =
            entry.map_err(|e| format!("Failed reading entry in {}: {e}", base.display()))?;
        let path = entry.path();
        let rel_path = rel.join(entry.file_name());
        if path.is_dir() {
            collect_recursive(&path, &rel_path, out)?;
        } else if path.is_file() || path.is_symlink() {
            out.insert(rel_path);
        }
    }
    Ok(())
}

fn classify_unmanaged(path: &Path) -> RuntimeEntryClassification {
    let lower = path.to_string_lossy().to_lowercase();
    let ignorable_prefixes = ["logs/", "shadercache/", "skse/", "f4se/"];
    if ignorable_prefixes.iter().any(|p| lower.starts_with(p))
        || [".log", ".tmp", ".bak", ".dmp"]
            .iter()
            .any(|ext| lower.ends_with(ext))
    {
        return RuntimeEntryClassification::UnmanagedIgnorable;
    }

    let adoptable_ext = [
        ".esp", ".esm", ".esl", ".nif", ".dds", ".pex", ".hkx", ".json", ".ini", ".txt",
    ];
    if adoptable_ext.iter().any(|ext| lower.ends_with(ext)) {
        RuntimeEntryClassification::UnmanagedAdoptable
    } else {
        RuntimeEntryClassification::UnknownNeedsReview
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::games::{Game, GameKind, GameLauncherSource, UmuGameConfig};
    use crate::core::mods::{GeneratedOutputPackage, ModDatabase, OwnedGeneratedFile};
    use std::sync::atomic::{AtomicU64, Ordering};
    use tempfile::TempDir;

    fn test_game(temp: &TempDir) -> Game {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = format!(
            "runtime_scan_test_{}",
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let root = temp.path().join("game_root");
        let data = root.join("Data");
        let mods_base = temp.path().join("mods_base");
        let prefix = temp.path().join("umu_prefix");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(mods_base.join("mods").join(&id)).unwrap();
        Game {
            id,
            name: "Test".to_string(),
            kind: GameKind::SkyrimSE,
            launcher_source: GameLauncherSource::NonSteamUmu,
            steam_app_id: None,
            root_path: root,
            data_path: data,
            mods_base_dir: Some(mods_base),
            umu_config: Some(UmuGameConfig {
                exe_path: PathBuf::from("game.exe"),
                prefix_path: Some(prefix),
                proton_path: None,
            }),
        }
    }

    #[test]
    fn classification_helper_is_truthful() {
        assert_eq!(
            classify_unmanaged(Path::new("textures/a.dds")),
            RuntimeEntryClassification::UnmanagedAdoptable
        );
        assert_eq!(
            classify_unmanaged(Path::new("logs/tool.log")),
            RuntimeEntryClassification::UnmanagedIgnorable
        );
        assert_eq!(
            classify_unmanaged(Path::new("meshes/custom.bin")),
            RuntimeEntryClassification::UnknownNeedsReview
        );
    }

    #[test]
    fn scan_is_profile_aware() {
        let temp = TempDir::new().unwrap();
        let game = test_game(&temp);
        std::fs::create_dir_all(game.data_path.join("textures")).unwrap();
        std::fs::write(game.data_path.join("textures/runtime.dds"), b"x").unwrap();
        let mut db = ModDatabase::default();

        db.active_profile_id = "profile_a".to_string();
        let a = scan_profile_runtime_changes(&game, &db).unwrap();
        assert!(
            a.entries
                .iter()
                .any(|e| e.relative_path == PathBuf::from("textures/runtime.dds"))
        );

        db.active_profile_id = "profile_b".to_string();
        db.profile_runtime_ignored.insert(
            "profile_b".to_string(),
            HashSet::from(["textures/runtime.dds".to_string()]),
        );
        let b = scan_profile_runtime_changes(&game, &db).unwrap();
        let entry = b
            .entries
            .iter()
            .find(|e| e.relative_path == PathBuf::from("textures/runtime.dds"))
            .unwrap();
        assert_eq!(entry.review_status, RuntimeEntryReviewStatus::Ignored);
    }

    #[test]
    fn adoptable_entries_can_be_ignored_and_resolved() {
        let temp = TempDir::new().unwrap();
        let game = test_game(&temp);
        std::fs::create_dir_all(game.data_path.join("textures")).unwrap();
        std::fs::write(game.data_path.join("textures/runtime.dds"), b"x").unwrap();
        let mut db = ModDatabase::default();

        let report = scan_profile_runtime_changes(&game, &db).unwrap();
        assert!(report.has_unresolved_changes());

        set_runtime_path_ignored(&mut db, Path::new("textures/runtime.dds"), true);
        let ignored = scan_profile_runtime_changes(&game, &db).unwrap();
        assert_eq!(ignored.unresolved_review_count(), 0);
    }

    #[test]
    fn managed_missing_is_detected() {
        let temp = TempDir::new().unwrap();
        let game = test_game(&temp);
        let mut db = ModDatabase::default();

        let mut package = GeneratedOutputPackage::new(
            "Generated",
            "tool_a",
            "default",
            db.active_profile_id.clone(),
            temp.path().join("pkg"),
        );
        package.owned_files.push(OwnedGeneratedFile {
            relative_path: PathBuf::from("meshes/out.nif"),
            captured_at: 0,
            source_tool: "tool_a".to_string(),
        });
        db.generated_outputs.push(package);

        let report = scan_profile_runtime_changes(&game, &db).unwrap();
        assert!(report.entries.iter().any(|e| {
            e.classification == RuntimeEntryClassification::ManagedOwnedMissing
                && e.relative_path == PathBuf::from("meshes/out.nif")
        }));
    }
}
