use crate::core::games::Game;
use crate::core::mods::{GeneratedOutputPackage, ModDatabase, OwnedGeneratedFile};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ToolRunContext {
    pub tool_id: String,
    pub run_profile: String,
}

#[derive(Debug, Clone, Default)]
pub struct FolderSnapshot {
    entries: HashMap<PathBuf, Vec<u8>>,
}

pub fn snapshot_game_data(game: &Game) -> Result<FolderSnapshot, String> {
    let mut snapshot = FolderSnapshot::default();
    collect_files(&game.data_path, Path::new(""), &mut snapshot.entries)?;
    Ok(snapshot)
}

pub fn register_output_directory_package(
    game: &Game,
    db: &mut ModDatabase,
    tool: &ToolRunContext,
    output_dir: &Path,
    name: &str,
) -> Result<String, String> {
    let package_id = replace_package_for_tool(game, db, tool, output_dir, name)?;
    Ok(package_id)
}

pub fn capture_and_register_from_game_data_diff(
    game: &Game,
    db: &mut ModDatabase,
    tool: &ToolRunContext,
    before: &FolderSnapshot,
    name: &str,
) -> Result<String, String> {
    let after = snapshot_game_data(game)?;
    let changes = diff_snapshots(before, &after);
    let stage_dir = game
        .mods_dir()
        .join("generated_outputs")
        .join(format!("{}_staging", tool.tool_id));
    if stage_dir.exists() {
        std::fs::remove_dir_all(&stage_dir)
            .map_err(|e| format!("Failed to clear previous generated output staging dir: {e}"))?;
    }
    for rel in &changes.created_or_modified {
        let source = game.data_path.join(rel);
        let target = stage_dir.join(rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create generated output staging parent: {e}"))?;
        }
        std::fs::copy(&source, &target)
            .map_err(|e| format!("Failed to copy generated output file into staging: {e}"))?;
    }

    // Restore or clean game folder now that output is captured.
    for rel in &changes.created_or_modified {
        let game_path = game.data_path.join(rel);
        if let Some(original) = before.entries.get(rel) {
            std::fs::write(&game_path, original).map_err(|e| {
                format!(
                    "Failed to restore pre-tool file {}: {e}",
                    game_path.display()
                )
            })?;
        } else if game_path.exists() {
            std::fs::remove_file(&game_path).map_err(|e| {
                format!(
                    "Failed to remove unmanaged generated file {}: {e}",
                    game_path.display()
                )
            })?;
        }
    }
    for rel in &changes.deleted {
        if let Some(original) = before.entries.get(rel) {
            let game_path = game.data_path.join(rel);
            if let Some(parent) = game_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to recreate deleted parent directory: {e}"))?;
            }
            std::fs::write(&game_path, original).map_err(|e| {
                format!(
                    "Failed to restore file deleted by tool {}: {e}",
                    game_path.display()
                )
            })?;
        }
    }

    replace_package_for_tool(game, db, tool, &stage_dir, name)
}

pub fn remove_generated_output_package(
    game: &Game,
    db: &mut ModDatabase,
    package_id: &str,
) -> Result<(), String> {
    if let Some(pkg) = db.generated_outputs.iter().find(|p| p.id == package_id)
        && pkg.source_path.exists()
    {
        std::fs::remove_dir_all(&pkg.source_path)
            .map_err(|e| format!("Failed to remove generated output package data: {e}"))?;
    }
    db.generated_outputs.retain(|p| p.id != package_id);
    db.save(game);
    Ok(())
}

pub fn cleanup_stale_generated_outputs(game: &Game, db: &mut ModDatabase) {
    db.generated_outputs.retain(|p| p.source_path.exists());
    db.save(game);
}

pub fn adopt_existing_game_data_files(
    game: &Game,
    db: &mut ModDatabase,
    tool: &ToolRunContext,
    files: &[PathBuf],
    package_name: &str,
) -> Result<String, String> {
    let staging = game
        .mods_dir()
        .join("generated_outputs")
        .join(format!("{}_adopt_staging", tool.tool_id));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)
            .map_err(|e| format!("Failed clearing previous adoption staging directory: {e}"))?;
    }
    for rel in files {
        let src = game.data_path.join(rel);
        if !src.is_file() {
            continue;
        }
        let dst = staging.join(rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed creating adoption parent directory: {e}"))?;
        }
        std::fs::copy(&src, &dst).map_err(|e| format!("Failed copying adopted file: {e}"))?;
        std::fs::remove_file(&src)
            .map_err(|e| format!("Failed cleaning adopted file from game folder: {e}"))?;
    }
    replace_package_for_tool(game, db, tool, &staging, package_name)
}

fn replace_package_for_tool(
    game: &Game,
    db: &mut ModDatabase,
    tool: &ToolRunContext,
    source_dir: &Path,
    name: &str,
) -> Result<String, String> {
    let dest = game
        .mods_dir()
        .join("generated_outputs")
        .join(format!("{}__{}", tool.tool_id, tool.run_profile));
    if dest.exists() {
        std::fs::remove_dir_all(&dest)
            .map_err(|e| format!("Failed to clear previous generated output package: {e}"))?;
    }
    copy_tree(source_dir, &dest)?;

    let mut package = GeneratedOutputPackage::new(
        name.to_string(),
        tool.tool_id.clone(),
        tool.run_profile.clone(),
        dest,
    );
    package.owned_files = enumerate_owned_files(&package)?;
    package.updated_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    db.generated_outputs
        .retain(|p| !(p.tool_id == tool.tool_id && p.run_profile == tool.run_profile));
    db.generated_outputs.push(package.clone());
    db.save(game);
    Ok(package.id)
}

fn enumerate_owned_files(
    package: &GeneratedOutputPackage,
) -> Result<Vec<OwnedGeneratedFile>, String> {
    let mut files = Vec::new();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut all = HashMap::new();
    collect_files(&package.source_path, Path::new(""), &mut all)?;
    for rel in all.keys() {
        files.push(OwnedGeneratedFile {
            relative_path: rel.clone(),
            captured_at: now,
            source_tool: package.tool_id.clone(),
        });
    }
    Ok(files)
}

#[derive(Debug, Default)]
struct SnapshotDiff {
    created_or_modified: HashSet<PathBuf>,
    deleted: HashSet<PathBuf>,
}

fn diff_snapshots(before: &FolderSnapshot, after: &FolderSnapshot) -> SnapshotDiff {
    let mut diff = SnapshotDiff::default();
    for (path, bytes) in &after.entries {
        match before.entries.get(path) {
            Some(previous) if previous == bytes => {}
            _ => {
                diff.created_or_modified.insert(path.clone());
            }
        }
    }
    for path in before.entries.keys() {
        if !after.entries.contains_key(path) {
            diff.deleted.insert(path.clone());
        }
    }
    diff
}

fn collect_files(
    root: &Path,
    rel: &Path,
    out: &mut HashMap<PathBuf, Vec<u8>>,
) -> Result<(), String> {
    if !root.exists() {
        return Ok(());
    }
    for entry in
        std::fs::read_dir(root).map_err(|e| format!("Failed reading {}: {e}", root.display()))?
    {
        let entry = entry.map_err(|e| format!("Failed reading directory entry: {e}"))?;
        let path = entry.path();
        let rel_path = rel.join(entry.file_name());
        if path.is_dir() {
            collect_files(&path, &rel_path, out)?;
        } else if path.is_file() {
            out.insert(
                rel_path,
                std::fs::read(&path)
                    .map_err(|e| format!("Failed reading {}: {e}", path.display()))?,
            );
        }
    }
    Ok(())
}

fn copy_tree(source: &Path, dest: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest)
        .map_err(|e| format!("Failed to create generated package destination: {e}"))?;
    for entry in std::fs::read_dir(source)
        .map_err(|e| format!("Failed to read generated source {}: {e}", source.display()))?
    {
        let entry = entry.map_err(|e| format!("Failed to read generated source entry: {e}"))?;
        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        if src_path.is_dir() {
            copy_tree(&src_path, &dest_path)?;
        } else if src_path.is_file() {
            if let Some(parent) = dest_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create generated package parent: {e}"))?;
            }
            std::fs::copy(&src_path, &dest_path)
                .map_err(|e| format!("Failed to copy generated file: {e}"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::deployment;
    use crate::core::games::{Game, GameKind, UmuGameConfig};
    use std::sync::atomic::{AtomicU64, Ordering};
    use tempfile::TempDir;

    fn test_game(temp: &TempDir) -> Game {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = format!(
            "generated_output_test_{}",
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let root = temp.path().join("game_root");
        let data = root.join("Data");
        let mods_base = temp.path().join("mods_base");
        let prefix = temp.path().join("umu_prefix");
        let plugins_dir = prefix
            .join("pfx")
            .join("drive_c")
            .join("users")
            .join("steamuser")
            .join("AppData")
            .join("Local")
            .join(GameKind::SkyrimSE.local_app_data_folder());
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(mods_base.join("mods").join(&id)).unwrap();
        std::fs::create_dir_all(&plugins_dir).unwrap();
        Game {
            id,
            name: "Test".to_string(),
            kind: GameKind::SkyrimSE,
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
    fn dedicated_output_registers_as_managed_package_and_deploys() {
        let temp = TempDir::new().unwrap();
        let game = test_game(&temp);
        let tool = ToolRunContext {
            tool_id: "bodyslide".to_string(),
            run_profile: "default".to_string(),
        };
        let explicit_output = temp.path().join("tool_output");
        std::fs::create_dir_all(explicit_output.join("meshes")).unwrap();
        std::fs::write(explicit_output.join("meshes/out.nif"), b"mesh").unwrap();

        let mut db = ModDatabase::default();
        register_output_directory_package(
            &game,
            &mut db,
            &tool,
            &explicit_output,
            "BodySlide Output",
        )
        .unwrap();
        deployment::rebuild_deployment(&game, &mut db).unwrap();

        assert!(game.data_path.join("meshes/out.nif").exists());
        assert_eq!(db.generated_outputs.len(), 1);
        assert_eq!(db.generated_outputs[0].tool_id, "bodyslide");
        assert_eq!(db.generated_outputs[0].owned_files.len(), 1);
    }

    #[test]
    fn rerun_replaces_previous_package_for_same_tool_profile() {
        let temp = TempDir::new().unwrap();
        let game = test_game(&temp);
        let tool = ToolRunContext {
            tool_id: "nemesis".to_string(),
            run_profile: "default".to_string(),
        };
        let explicit_output = temp.path().join("tool_output");
        std::fs::create_dir_all(explicit_output.join("meshes")).unwrap();
        std::fs::write(explicit_output.join("meshes/out.nif"), b"v1").unwrap();

        let mut db = ModDatabase::default();
        register_output_directory_package(
            &game,
            &mut db,
            &tool,
            &explicit_output,
            "Nemesis Output",
        )
        .unwrap();
        std::fs::write(explicit_output.join("meshes/out.nif"), b"v2").unwrap();
        register_output_directory_package(
            &game,
            &mut db,
            &tool,
            &explicit_output,
            "Nemesis Output",
        )
        .unwrap();

        assert_eq!(db.generated_outputs.len(), 1);
        let content = std::fs::read(
            db.generated_outputs[0]
                .source_path
                .join("meshes")
                .join("out.nif"),
        )
        .unwrap();
        assert_eq!(content, b"v2");
    }

    #[test]
    fn snapshot_diff_capture_restores_game_folder_and_registers_output() {
        let temp = TempDir::new().unwrap();
        let game = test_game(&temp);
        let tool = ToolRunContext {
            tool_id: "pandora".to_string(),
            run_profile: "default".to_string(),
        };
        std::fs::create_dir_all(game.data_path.join("meshes")).unwrap();
        std::fs::write(game.data_path.join("meshes/existing.nif"), b"old").unwrap();
        let before = snapshot_game_data(&game).unwrap();

        std::fs::write(game.data_path.join("meshes/existing.nif"), b"new").unwrap();
        std::fs::write(game.data_path.join("meshes/newgen.nif"), b"generated").unwrap();

        let mut db = ModDatabase::default();
        capture_and_register_from_game_data_diff(&game, &mut db, &tool, &before, "Pandora Output")
            .unwrap();

        assert_eq!(
            std::fs::read(game.data_path.join("meshes/existing.nif")).unwrap(),
            b"old"
        );
        assert!(!game.data_path.join("meshes/newgen.nif").exists());
        assert_eq!(db.generated_outputs.len(), 1);
        assert_eq!(db.generated_outputs[0].owned_files.len(), 2);
    }

    #[test]
    fn generated_plugin_output_is_written_to_plugins_txt_on_rebuild() {
        let temp = TempDir::new().unwrap();
        let game = test_game(&temp);
        let tool = ToolRunContext {
            tool_id: "xedit".to_string(),
            run_profile: "default".to_string(),
        };
        let explicit_output = temp.path().join("tool_output");
        std::fs::create_dir_all(&explicit_output).unwrap();
        std::fs::write(explicit_output.join("GeneratedPatch.esp"), b"patch").unwrap();

        let mut db = ModDatabase::default();
        register_output_directory_package(&game, &mut db, &tool, &explicit_output, "xEdit Output").unwrap();
        deployment::rebuild_deployment(&game, &mut db).unwrap();

        let plugins = std::fs::read_to_string(game.plugins_txt_path().unwrap()).unwrap();
        assert!(plugins.contains("*GeneratedPatch.esp"));
    }

    #[test]
    fn remove_generated_output_package_cleans_deployment_links() {
        let temp = TempDir::new().unwrap();
        let game = test_game(&temp);
        let tool = ToolRunContext {
            tool_id: "bodyslide".to_string(),
            run_profile: "default".to_string(),
        };
        let explicit_output = temp.path().join("tool_output");
        std::fs::create_dir_all(explicit_output.join("meshes")).unwrap();
        std::fs::write(explicit_output.join("meshes/out.nif"), b"mesh").unwrap();
        let mut db = ModDatabase::default();
        let package_id = register_output_directory_package(
            &game,
            &mut db,
            &tool,
            &explicit_output,
            "BodySlide Output",
        )
        .unwrap();
        deployment::rebuild_deployment(&game, &mut db).unwrap();
        assert!(game.data_path.join("meshes/out.nif").exists());

        remove_generated_output_package(&game, &mut db, &package_id).unwrap();
        deployment::rebuild_deployment(&game, &mut db).unwrap();
        assert!(!game.data_path.join("meshes/out.nif").exists());
    }
}
