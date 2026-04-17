use crate::core::config::{ToolConfig, ToolOutputMode, ToolPresetKind, ToolRunProfile};
use crate::core::games::Game;
use crate::core::mods::ModDatabase;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputClass {
    Asset,
    Plugin,
}

#[derive(Debug, Clone)]
pub struct PreflightReport {
    pub warnings: Vec<String>,
}

pub trait ToolAdapter {
    fn id(&self) -> &'static str;
    fn default_profiles(&self, tool: &ToolConfig) -> Vec<ToolRunProfile>;
    fn validate(
        &self,
        game: &Game,
        tool: &ToolConfig,
        profile: &ToolRunProfile,
    ) -> Result<PreflightReport, String>;
    fn classify_output(&self, rel_path: &Path) -> OutputClass;
    fn detect_unmanaged(
        &self,
        game: &Game,
        db: &ModDatabase,
        profile: &ToolRunProfile,
    ) -> Result<Vec<PathBuf>, String>;
}

pub fn adapter_for_tool(tool: &ToolConfig) -> Box<dyn ToolAdapter> {
    match tool.preset {
        ToolPresetKind::BodySlide => Box::new(BodySlideAdapter),
        ToolPresetKind::Pandora => Box::new(PandoraAdapter),
        ToolPresetKind::Nemesis => Box::new(NemesisAdapter),
        ToolPresetKind::Generic => Box::new(GenericAdapter),
    }
}

struct GenericAdapter;
struct BodySlideAdapter;
struct PandoraAdapter;
struct NemesisAdapter;

impl ToolAdapter for GenericAdapter {
    fn id(&self) -> &'static str {
        "generic"
    }

    fn default_profiles(&self, tool: &ToolConfig) -> Vec<ToolRunProfile> {
        vec![ToolRunProfile {
            id: "default".to_string(),
            name: "Default".to_string(),
            output_mode: ToolOutputMode::SnapshotGameData,
            managed_output_dir: None,
            generated_package_name: format!("{} Output", tool.name),
        }]
    }

    fn validate(
        &self,
        game: &Game,
        tool: &ToolConfig,
        profile: &ToolRunProfile,
    ) -> Result<PreflightReport, String> {
        validate_common(game, tool, profile)
    }

    fn classify_output(&self, rel_path: &Path) -> OutputClass {
        classify_by_extension(rel_path)
    }

    fn detect_unmanaged(
        &self,
        game: &Game,
        db: &ModDatabase,
        _profile: &ToolRunProfile,
    ) -> Result<Vec<PathBuf>, String> {
        detect_unowned_paths(game, db, |_rel| true)
    }
}

impl ToolAdapter for BodySlideAdapter {
    fn id(&self) -> &'static str {
        "bodyslide"
    }
    fn default_profiles(&self, _tool: &ToolConfig) -> Vec<ToolRunProfile> {
        vec![ToolRunProfile {
            id: "bodyslide_default".to_string(),
            name: "BodySlide Output".to_string(),
            output_mode: ToolOutputMode::DedicatedDirectory,
            managed_output_dir: None,
            generated_package_name: "BodySlide Output".to_string(),
        }]
    }
    fn validate(
        &self,
        game: &Game,
        tool: &ToolConfig,
        profile: &ToolRunProfile,
    ) -> Result<PreflightReport, String> {
        let mut report = validate_common(game, tool, profile)?;
        if tool
            .exe_path
            .to_string_lossy()
            .to_lowercase()
            .contains("bodyslide")
            .not()
        {
            report
                .warnings
                .push("Executable path does not look like BodySlide".to_string());
        }
        Ok(report)
    }
    fn classify_output(&self, rel_path: &Path) -> OutputClass {
        classify_by_extension(rel_path)
    }
    fn detect_unmanaged(
        &self,
        game: &Game,
        db: &ModDatabase,
        _profile: &ToolRunProfile,
    ) -> Result<Vec<PathBuf>, String> {
        detect_unowned_paths(game, db, |rel| {
            rel.starts_with("meshes") || rel.starts_with("textures")
        })
    }
}

impl ToolAdapter for PandoraAdapter {
    fn id(&self) -> &'static str {
        "pandora"
    }
    fn default_profiles(&self, _tool: &ToolConfig) -> Vec<ToolRunProfile> {
        vec![ToolRunProfile {
            id: "pandora_default".to_string(),
            name: "Pandora Output".to_string(),
            output_mode: ToolOutputMode::SnapshotGameData,
            managed_output_dir: None,
            generated_package_name: "Pandora Output".to_string(),
        }]
    }
    fn validate(
        &self,
        game: &Game,
        tool: &ToolConfig,
        profile: &ToolRunProfile,
    ) -> Result<PreflightReport, String> {
        validate_common(game, tool, profile)
    }
    fn classify_output(&self, rel_path: &Path) -> OutputClass {
        classify_by_extension(rel_path)
    }
    fn detect_unmanaged(
        &self,
        game: &Game,
        db: &ModDatabase,
        _profile: &ToolRunProfile,
    ) -> Result<Vec<PathBuf>, String> {
        detect_unowned_paths(game, db, |rel| {
            rel.to_string_lossy().to_lowercase().contains("pandora")
                || rel.to_string_lossy().to_lowercase().contains("animation")
        })
    }
}

impl ToolAdapter for NemesisAdapter {
    fn id(&self) -> &'static str {
        "nemesis"
    }
    fn default_profiles(&self, _tool: &ToolConfig) -> Vec<ToolRunProfile> {
        vec![ToolRunProfile {
            id: "nemesis_default".to_string(),
            name: "Nemesis Output".to_string(),
            output_mode: ToolOutputMode::SnapshotGameData,
            managed_output_dir: None,
            generated_package_name: "Nemesis Output".to_string(),
        }]
    }
    fn validate(
        &self,
        game: &Game,
        tool: &ToolConfig,
        profile: &ToolRunProfile,
    ) -> Result<PreflightReport, String> {
        validate_common(game, tool, profile)
    }
    fn classify_output(&self, rel_path: &Path) -> OutputClass {
        classify_by_extension(rel_path)
    }
    fn detect_unmanaged(
        &self,
        game: &Game,
        db: &ModDatabase,
        _profile: &ToolRunProfile,
    ) -> Result<Vec<PathBuf>, String> {
        detect_unowned_paths(game, db, |rel| {
            rel.to_string_lossy().to_lowercase().contains("nemesis")
                || rel.to_string_lossy().to_lowercase().contains("behavior")
        })
    }
}

fn validate_common(
    game: &Game,
    tool: &ToolConfig,
    profile: &ToolRunProfile,
) -> Result<PreflightReport, String> {
    if !tool.exe_path.is_file() {
        return Err(format!(
            "Tool executable does not exist or is not a file: {}",
            tool.exe_path.display()
        ));
    }
    if !game.root_path.is_dir() || !game.data_path.is_dir() {
        return Err(format!(
            "Game root/data directories are invalid: root={}, data={}",
            game.root_path.display(),
            game.data_path.display()
        ));
    }
    if profile.id.trim().is_empty() || profile.name.trim().is_empty() {
        return Err("Run profile id/name must not be empty".to_string());
    }
    if profile.output_mode == ToolOutputMode::DedicatedDirectory {
        let out = profile
            .managed_output_dir
            .as_ref()
            .ok_or_else(|| "Dedicated output mode requires managed output directory".to_string())?;
        if out.exists() && !out.is_dir() {
            return Err(format!(
                "Managed output path exists but is not a directory: {}",
                out.display()
            ));
        }
    }
    Ok(PreflightReport { warnings: vec![] })
}

fn classify_by_extension(rel_path: &Path) -> OutputClass {
    let lower = rel_path.to_string_lossy().to_lowercase();
    if lower.ends_with(".esp") || lower.ends_with(".esm") || lower.ends_with(".esl") {
        OutputClass::Plugin
    } else {
        OutputClass::Asset
    }
}

fn detect_unowned_paths<F>(game: &Game, db: &ModDatabase, filter: F) -> Result<Vec<PathBuf>, String>
where
    F: Fn(&Path) -> bool,
{
    let mut owned: HashSet<PathBuf> = HashSet::new();
    for pkg in &db.generated_outputs {
        for file in &pkg.owned_files {
            owned.insert(file.relative_path.clone());
        }
    }
    let mut unmanaged = Vec::new();
    collect_unmanaged(
        &game.data_path,
        Path::new(""),
        &owned,
        &filter,
        &mut unmanaged,
    )?;
    Ok(unmanaged)
}

fn collect_unmanaged<F>(
    root: &Path,
    rel: &Path,
    owned: &HashSet<PathBuf>,
    filter: &F,
    out: &mut Vec<PathBuf>,
) -> Result<(), String>
where
    F: Fn(&Path) -> bool,
{
    for entry in
        std::fs::read_dir(root).map_err(|e| format!("Failed reading {}: {e}", root.display()))?
    {
        let entry = entry.map_err(|e| format!("Failed reading directory entry: {e}"))?;
        let p = entry.path();
        let rel_path = rel.join(entry.file_name());
        if p.is_dir() {
            collect_unmanaged(&p, &rel_path, owned, filter, out)?;
        } else if p.is_file() && !owned.contains(&rel_path) && filter(&rel_path) {
            out.push(rel_path);
        }
    }
    Ok(())
}

trait BoolExt {
    fn not(self) -> bool;
}
impl BoolExt for bool {
    fn not(self) -> bool {
        !self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::games::{Game, GameKind, GameLauncherSource, UmuGameConfig};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn game(tmp: &TempDir) -> Game {
        let root = tmp.path().join("game");
        let data = root.join("Data");
        std::fs::create_dir_all(&data).unwrap();
        Game {
            id: "adapter_test".to_string(),
            name: "Adapter Test".to_string(),
            kind: GameKind::SkyrimSE,
            launcher_source: GameLauncherSource::NonSteamUmu,
            steam_app_id: None,
            root_path: root,
            data_path: data,
            mods_base_dir: Some(tmp.path().join("mods")),
            umu_config: Some(UmuGameConfig {
                exe_path: PathBuf::from("game.exe"),
                prefix_path: Some(tmp.path().join("prefix")),
                proton_path: None,
            }),
        }
    }

    #[test]
    fn preflight_blocks_missing_executable() {
        let tmp = TempDir::new().unwrap();
        let game = game(&tmp);
        let tool = ToolConfig {
            id: "b".to_string(),
            name: "BodySlide".to_string(),
            exe_path: tmp.path().join("missing.exe"),
            arguments: String::new(),
            app_id: 489830,
            preset: ToolPresetKind::BodySlide,
            run_profiles: vec![],
        };
        let adapter = adapter_for_tool(&tool);
        let profile = adapter.default_profiles(&tool).remove(0);
        let err = adapter.validate(&game, &tool, &profile).unwrap_err();
        assert!(err.contains("does not exist"));
    }

    #[test]
    fn adapters_provide_expected_default_profiles() {
        let tool = ToolConfig {
            id: "n".to_string(),
            name: "Nemesis".to_string(),
            exe_path: PathBuf::from("nemesis.exe"),
            arguments: String::new(),
            app_id: 489830,
            preset: ToolPresetKind::Nemesis,
            run_profiles: vec![],
        };
        let adapter = adapter_for_tool(&tool);
        let profiles = adapter.default_profiles(&tool);
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].generated_package_name, "Nemesis Output");
    }
}
