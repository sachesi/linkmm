use crate::core::config::{ToolConfig, ToolOutputMode, ToolRunProfile};
use crate::core::deployment;
use crate::core::games::Game;
use crate::core::generated_outputs::{
    capture_and_register_from_game_data_diff, register_output_directory_package, snapshot_game_data,
    ToolRunContext,
};
use crate::core::mods::ModDatabase;

#[derive(Debug, Clone)]
pub struct ToolRunResult {
    pub package_id: Option<String>,
}

pub fn run_tool_with_managed_outputs<F>(
    game: &Game,
    db: &mut ModDatabase,
    tool: &ToolConfig,
    profile: &ToolRunProfile,
    execute: F,
) -> Result<ToolRunResult, String>
where
    F: FnOnce(&ToolConfig, &ToolRunProfile) -> Result<std::process::ExitStatus, String>,
{
    let snapshot = match profile.output_mode {
        ToolOutputMode::SnapshotGameData => Some(snapshot_game_data(game)?),
        ToolOutputMode::DedicatedDirectory => None,
    };

    let status = execute(tool, profile)?;
    if !status.success() {
        return Err(format!(
            "Tool {} exited with non-zero status: {}",
            tool.name, status
        ));
    }

    let run_ctx = ToolRunContext {
        tool_id: tool.id.clone(),
        run_profile: profile.id.clone(),
    };
    let package_name = if profile.generated_package_name.trim().is_empty() {
        format!("{} ({})", tool.name, profile.name)
    } else {
        profile.generated_package_name.clone()
    };

    let package_id = match profile.output_mode {
        ToolOutputMode::DedicatedDirectory => {
            let output_dir = profile
                .managed_output_dir
                .as_ref()
                .ok_or_else(|| "Dedicated output mode requires a managed output directory".to_string())?;
            Some(register_output_directory_package(
                game,
                db,
                &run_ctx,
                output_dir,
                &package_name,
            )?)
        }
        ToolOutputMode::SnapshotGameData => Some(capture_and_register_from_game_data_diff(
            game,
            db,
            &run_ctx,
            snapshot
                .as_ref()
                .ok_or_else(|| "Snapshot state missing for snapshot output mode".to_string())?,
            &package_name,
        )?),
    };

    deployment::rebuild_deployment(game, db)?;
    db.save(game);

    Ok(ToolRunResult { package_id })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::games::{GameKind, UmuGameConfig};
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tempfile::TempDir;

    fn test_game(temp: &TempDir) -> Game {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = format!("tool_run_test_{}", COUNTER.fetch_add(1, Ordering::Relaxed));
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
        std::fs::create_dir_all(plugins_dir).unwrap();
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

    fn test_tool(profile: ToolRunProfile) -> ToolConfig {
        ToolConfig {
            id: "tool_a".to_string(),
            name: "ToolA".to_string(),
            exe_path: PathBuf::from("tool.exe"),
            arguments: String::new(),
            app_id: 489830,
            run_profiles: vec![profile],
        }
    }

    #[test]
    fn explicit_output_run_imports_output_and_rebuilds() {
        let temp = TempDir::new().unwrap();
        let game = test_game(&temp);
        let output = temp.path().join("tool_output");
        std::fs::create_dir_all(output.join("meshes")).unwrap();
        std::fs::write(output.join("meshes/gen.nif"), b"mesh").unwrap();
        let profile = ToolRunProfile {
            id: "default".to_string(),
            name: "Default".to_string(),
            output_mode: ToolOutputMode::DedicatedDirectory,
            managed_output_dir: Some(output),
            generated_package_name: "ToolA Output".to_string(),
        };
        let tool = test_tool(profile.clone());
        let mut db = ModDatabase::default();

        run_tool_with_managed_outputs(&game, &mut db, &tool, &profile, |_tool, _profile| {
            Ok(std::process::ExitStatus::from_raw(0))
        })
        .unwrap();
        assert!(game.data_path.join("meshes/gen.nif").exists());
        assert_eq!(db.generated_outputs.len(), 1);
    }

    #[test]
    fn failed_run_does_not_create_generated_output_package() {
        let temp = TempDir::new().unwrap();
        let game = test_game(&temp);
        let profile = ToolRunProfile {
            id: "default".to_string(),
            name: "Default".to_string(),
            output_mode: ToolOutputMode::SnapshotGameData,
            managed_output_dir: None,
            generated_package_name: "ToolA Output".to_string(),
        };
        let tool = test_tool(profile.clone());
        let mut db = ModDatabase::default();
        let err = run_tool_with_managed_outputs(&game, &mut db, &tool, &profile, |_tool, _profile| {
            Ok(std::process::ExitStatus::from_raw(1))
        })
        .unwrap_err();
        assert!(err.contains("non-zero"));
        assert!(db.generated_outputs.is_empty());
    }
}
