use crate::core::games::Game;
use crate::core::mods::ModDatabase;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

const WORKSPACE_BASELINE_FILE: &str = "workspace_baseline.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeploymentState {
    Deployed,
    NotDeployed,
    Dirty,
    Busy,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceOperation {
    None,
    Install,
    Deploy,
    ToolRun,
    Capture,
    Restore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PendingChanges {
    pub mod_set_changed: bool,
    pub mod_enabled_changed: bool,
    pub mod_order_changed: bool,
    pub plugin_order_changed: bool,
    pub generated_outputs_changed: bool,
    pub unmanaged_runtime_changed: bool,
}

impl PendingChanges {
    pub fn any(&self) -> bool {
        self.mod_set_changed
            || self.mod_enabled_changed
            || self.mod_order_changed
            || self.plugin_order_changed
            || self.generated_outputs_changed
            || self.unmanaged_runtime_changed
    }

    pub fn reasons(&self) -> Vec<&'static str> {
        let mut reasons = Vec::new();
        if self.mod_set_changed {
            reasons.push("Installed mods changed");
        }
        if self.mod_enabled_changed {
            reasons.push("Enabled/disabled mods changed");
        }
        if self.mod_order_changed {
            reasons.push("Mod order changed");
        }
        if self.plugin_order_changed {
            reasons.push("Plugin load order changed");
        }
        if self.generated_outputs_changed {
            reasons.push("Generated outputs changed");
        }
        if self.unmanaged_runtime_changed {
            reasons.push("Runtime/unmanaged files detected");
        }
        reasons
    }
}

#[derive(Debug, Clone)]
pub struct WorkspaceState {
    pub game_id: String,
    pub profile_id: String,
    pub deployment_state: DeploymentState,
    pub pending_changes: PendingChanges,
    pub current_operation: WorkspaceOperation,
    pub status_message: Option<String>,
    pub status_severity: StatusSeverity,
    pub safe_redeploy_required: bool,
    pub safe_redeploy_recommended: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
struct WorkspaceSnapshot {
    mod_ids: Vec<String>,
    enabled_mod_ids: BTreeSet<String>,
    mod_order: Vec<String>,
    plugin_order: Vec<String>,
    plugin_disabled: BTreeSet<String>,
    generated_outputs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct WorkspaceBaseline {
    profile_id: String,
    snapshot: WorkspaceSnapshot,
}

#[derive(Debug, Clone)]
struct WorkspaceRuntimeState {
    operation: WorkspaceOperation,
    status_message: Option<String>,
    severity: StatusSeverity,
    deploy_failed: bool,
    unmanaged_runtime_changed: bool,
}

impl Default for WorkspaceRuntimeState {
    fn default() -> Self {
        Self {
            operation: WorkspaceOperation::None,
            status_message: None,
            severity: StatusSeverity::Info,
            deploy_failed: false,
            unmanaged_runtime_changed: false,
        }
    }
}

static RUNTIME_STATE: OnceLock<Mutex<HashMap<String, WorkspaceRuntimeState>>> = OnceLock::new();

fn runtime_state_map() -> &'static Mutex<HashMap<String, WorkspaceRuntimeState>> {
    RUNTIME_STATE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn baseline_path(game: &Game, profile_id: &str) -> PathBuf {
    game.config_dir()
        .join("profiles")
        .join(profile_id)
        .join(WORKSPACE_BASELINE_FILE)
}

fn load_baseline(game: &Game, profile_id: &str) -> Option<WorkspaceBaseline> {
    let raw = std::fs::read_to_string(baseline_path(game, profile_id)).ok()?;
    toml::from_str(&raw).ok()
}

fn save_baseline(game: &Game, baseline: &WorkspaceBaseline) -> Result<(), String> {
    let path = baseline_path(game, &baseline.profile_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed creating workspace baseline directory: {e}"))?;
    }
    let raw = toml::to_string_pretty(baseline)
        .map_err(|e| format!("Failed serializing workspace baseline: {e}"))?;
    std::fs::write(path, raw).map_err(|e| format!("Failed writing workspace baseline: {e}"))
}

fn snapshot_from_db(db: &ModDatabase) -> WorkspaceSnapshot {
    let mut mod_ids = db.mods.iter().map(|m| m.id.clone()).collect::<Vec<_>>();
    mod_ids.sort();

    let enabled_mod_ids = db
        .mods
        .iter()
        .filter(|m| m.enabled)
        .map(|m| m.id.clone())
        .collect::<BTreeSet<_>>();

    let mod_order = db.mods.iter().map(|m| m.id.clone()).collect::<Vec<_>>();

    let mut generated_outputs = db
        .generated_outputs
        .iter()
        .filter(|o| o.manager_profile_id == db.active_profile_id)
        .map(|o| format!("{}:{}:{}", o.id, o.enabled, o.updated_at))
        .collect::<Vec<_>>();
    generated_outputs.sort();

    WorkspaceSnapshot {
        mod_ids,
        enabled_mod_ids,
        mod_order,
        plugin_order: db.plugin_load_order.clone(),
        plugin_disabled: db.plugin_disabled.clone().into_iter().collect(),
        generated_outputs,
    }
}

fn compute_pending_changes(
    baseline: Option<&WorkspaceBaseline>,
    db: &ModDatabase,
    runtime: &WorkspaceRuntimeState,
) -> PendingChanges {
    let Some(baseline) = baseline else {
        return PendingChanges {
            unmanaged_runtime_changed: runtime.unmanaged_runtime_changed,
            ..PendingChanges::default()
        };
    };

    let now = snapshot_from_db(db);
    PendingChanges {
        mod_set_changed: now.mod_ids != baseline.snapshot.mod_ids,
        mod_enabled_changed: now.enabled_mod_ids != baseline.snapshot.enabled_mod_ids,
        mod_order_changed: now.mod_order != baseline.snapshot.mod_order,
        plugin_order_changed: now.plugin_order != baseline.snapshot.plugin_order
            || now.plugin_disabled != baseline.snapshot.plugin_disabled,
        generated_outputs_changed: now.generated_outputs != baseline.snapshot.generated_outputs,
        unmanaged_runtime_changed: runtime.unmanaged_runtime_changed,
    }
}

pub fn mark_operation(game_id: &str, op: WorkspaceOperation) {
    let mut map = runtime_state_map().lock().expect("workspace lock poisoned");
    map.entry(game_id.to_string()).or_default().operation = op;
}

pub fn set_status(game_id: &str, severity: StatusSeverity, message: impl Into<String>) {
    let mut map = runtime_state_map().lock().expect("workspace lock poisoned");
    let state = map.entry(game_id.to_string()).or_default();
    state.severity = severity;
    state.status_message = Some(message.into());
}

pub fn clear_status(game_id: &str) {
    let mut map = runtime_state_map().lock().expect("workspace lock poisoned");
    if let Some(state) = map.get_mut(game_id) {
        state.status_message = None;
        state.severity = StatusSeverity::Info;
    }
}

pub fn mark_unmanaged_runtime_changes(game_id: &str, changed: bool) {
    let mut map = runtime_state_map().lock().expect("workspace lock poisoned");
    map.entry(game_id.to_string())
        .or_default()
        .unmanaged_runtime_changed = changed;
}

pub fn mark_deploy_failure(game_id: &str, message: impl Into<String>) {
    let mut map = runtime_state_map().lock().expect("workspace lock poisoned");
    let state = map.entry(game_id.to_string()).or_default();
    state.deploy_failed = true;
    state.severity = StatusSeverity::Error;
    state.status_message = Some(message.into());
    state.operation = WorkspaceOperation::None;
}

pub fn mark_deployed_clean(game: &Game, db: &ModDatabase) -> Result<(), String> {
    let baseline = WorkspaceBaseline {
        profile_id: db.active_profile_id.clone(),
        snapshot: snapshot_from_db(db),
    };
    save_baseline(game, &baseline)?;

    let mut map = runtime_state_map().lock().expect("workspace lock poisoned");
    let state = map.entry(game.id.clone()).or_default();
    state.deploy_failed = false;
    state.operation = WorkspaceOperation::None;
    state.unmanaged_runtime_changed = false;
    state.status_message = Some("Deployment is up to date".to_string());
    state.severity = StatusSeverity::Info;
    Ok(())
}

pub fn profile_switch_policy(state: &WorkspaceState) -> ProfileSwitchPolicy {
    if state.current_operation != WorkspaceOperation::None {
        return ProfileSwitchPolicy::Blocked(
            "Cannot switch profile while an operation is running".to_string(),
        );
    }
    if state.safe_redeploy_required {
        return ProfileSwitchPolicy::Warn("Current profile has undeployed changes".to_string());
    }
    ProfileSwitchPolicy::Allowed
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileSwitchPolicy {
    Allowed,
    Warn(String),
    Blocked(String),
}

pub fn workspace_state_for_game(game: &Game) -> WorkspaceState {
    let db = ModDatabase::load(game);
    let profile_id = db.active_profile_id.clone();
    let baseline = load_baseline(game, &profile_id);
    let runtime = runtime_state_map()
        .lock()
        .expect("workspace lock poisoned")
        .get(&game.id)
        .cloned()
        .unwrap_or_default();

    let pending_changes = compute_pending_changes(baseline.as_ref(), &db, &runtime);
    let dirty = pending_changes.any();
    let deployment_state = if runtime.operation == WorkspaceOperation::Deploy {
        DeploymentState::Busy
    } else if runtime.deploy_failed {
        DeploymentState::Failed
    } else if dirty {
        DeploymentState::Dirty
    } else if baseline.is_some() {
        DeploymentState::Deployed
    } else {
        DeploymentState::NotDeployed
    };

    WorkspaceState {
        game_id: game.id.clone(),
        profile_id,
        deployment_state,
        pending_changes: pending_changes.clone(),
        current_operation: runtime.operation,
        status_message: runtime.status_message.clone(),
        status_severity: runtime.severity,
        safe_redeploy_required: dirty || runtime.deploy_failed,
        safe_redeploy_recommended: dirty,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::games::{Game, GameKind, GameLauncherSource, UmuGameConfig};
    use tempfile::TempDir;

    fn test_game(temp: &TempDir, id: &str) -> Game {
        let root = temp.path().join("game_root");
        let data = root.join("Data");
        let mods_base = temp.path().join("mods_base");
        let prefix = temp.path().join("umu_prefix");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(mods_base.join("mods").join(id)).unwrap();
        Game {
            id: id.to_string(),
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
    fn dirty_state_transitions_across_deploy_baseline() {
        let temp = TempDir::new().unwrap();
        let game = test_game(&temp, "workspace_dirty_transitions");
        let mut db = ModDatabase::default();
        db.save(&game);

        let initial = workspace_state_for_game(&game);
        assert_eq!(initial.deployment_state, DeploymentState::NotDeployed);

        mark_deployed_clean(&game, &db).unwrap();
        let clean = workspace_state_for_game(&game);
        assert_eq!(clean.deployment_state, DeploymentState::Deployed);
        assert!(!clean.safe_redeploy_required);

        db.plugin_load_order.push("MyPatch.esp".to_string());
        db.save(&game);
        let dirty = workspace_state_for_game(&game);
        assert_eq!(dirty.deployment_state, DeploymentState::Dirty);
        assert!(dirty.pending_changes.plugin_order_changed);
        assert!(dirty.safe_redeploy_required);
    }

    #[test]
    fn deploy_state_tracks_busy_and_failed_runtime_states() {
        let temp = TempDir::new().unwrap();
        let game = test_game(&temp, "workspace_deploy_states");
        let db = ModDatabase::default();
        db.save(&game);
        mark_deployed_clean(&game, &db).unwrap();

        mark_operation(&game.id, WorkspaceOperation::Deploy);
        let busy = workspace_state_for_game(&game);
        assert_eq!(busy.deployment_state, DeploymentState::Busy);

        mark_deploy_failure(&game.id, "deploy failed");
        let failed = workspace_state_for_game(&game);
        assert_eq!(failed.deployment_state, DeploymentState::Failed);
        assert!(failed.status_message.unwrap_or_default().contains("failed"));
    }

    #[test]
    fn profile_switch_policy_blocks_on_operation_and_warns_on_dirty() {
        let state_busy = WorkspaceState {
            game_id: "g".to_string(),
            profile_id: "p".to_string(),
            deployment_state: DeploymentState::Busy,
            pending_changes: PendingChanges::default(),
            current_operation: WorkspaceOperation::ToolRun,
            status_message: None,
            status_severity: StatusSeverity::Info,
            safe_redeploy_required: false,
            safe_redeploy_recommended: false,
        };
        assert!(matches!(
            profile_switch_policy(&state_busy),
            ProfileSwitchPolicy::Blocked(_)
        ));

        let state_dirty = WorkspaceState {
            current_operation: WorkspaceOperation::None,
            safe_redeploy_required: true,
            ..state_busy
        };
        assert!(matches!(
            profile_switch_policy(&state_dirty),
            ProfileSwitchPolicy::Warn(_)
        ));
    }

    #[test]
    fn generated_output_change_is_detected_against_baseline() {
        let temp = TempDir::new().unwrap();
        let game = test_game(&temp, "workspace_generated_changes");
        let mut db = ModDatabase::default();
        db.save(&game);
        mark_deployed_clean(&game, &db).unwrap();

        db.generated_outputs
            .push(crate::core::mods::GeneratedOutputPackage::new(
                "Tool Output",
                "tool",
                "default",
                db.active_profile_id.clone(),
                temp.path().join("gen"),
            ));
        db.save(&game);

        let state = workspace_state_for_game(&game);
        assert!(state.pending_changes.generated_outputs_changed);
    }
}
