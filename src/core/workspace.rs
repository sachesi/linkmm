use crate::core::games::Game;
use crate::core::mods::ModDatabase;
use crate::core::runtime_scan::RuntimeScanReport;
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
    pub pending_destructive_changes: bool,
}

impl PendingChanges {
    pub fn any(&self) -> bool {
        self.mod_set_changed
            || self.mod_enabled_changed
            || self.mod_order_changed
            || self.plugin_order_changed
            || self.generated_outputs_changed
            || self.unmanaged_runtime_changed
            || self.pending_destructive_changes
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
        if self.pending_destructive_changes {
            reasons.push("Pending destructive cleanup");
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
    pub runtime_review_required: bool,
    pub runtime_adoptable_pending: usize,
    pub runtime_unknown_pending: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
struct WorkspaceSnapshot {
    mod_ids: Vec<String>,
    enabled_mod_ids: BTreeSet<String>,
    mod_order: Vec<String>,
    plugin_order: Vec<String>,
    plugin_disabled: BTreeSet<String>,
    generated_outputs: Vec<String>,
    pending_destructive: Vec<String>,
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
    runtime_scan: RuntimeScanStatus,
}

#[derive(Debug, Clone, Default)]
struct RuntimeScanStatus {
    unresolved_review_count: usize,
    adoptable_count: usize,
    unknown_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct WorkspaceRuntimeKey {
    game_id: String,
    profile_id: String,
}

impl Default for WorkspaceRuntimeState {
    fn default() -> Self {
        Self {
            operation: WorkspaceOperation::None,
            status_message: None,
            severity: StatusSeverity::Info,
            deploy_failed: false,
            unmanaged_runtime_changed: false,
            runtime_scan: RuntimeScanStatus::default(),
        }
    }
}

static RUNTIME_STATE: OnceLock<Mutex<HashMap<WorkspaceRuntimeKey, WorkspaceRuntimeState>>> =
    OnceLock::new();

fn runtime_state_map() -> &'static Mutex<HashMap<WorkspaceRuntimeKey, WorkspaceRuntimeState>> {
    RUNTIME_STATE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn runtime_key(game_id: &str, profile_id: &str) -> WorkspaceRuntimeKey {
    WorkspaceRuntimeKey {
        game_id: game_id.to_string(),
        profile_id: profile_id.to_string(),
    }
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
    let mut pending_destructive = db
        .mods
        .iter()
        .filter(|m| m.pending_removal)
        .map(|m| format!("mod:{}", m.id))
        .chain(
            db.generated_outputs
                .iter()
                .filter(|p| p.pending_removal && p.manager_profile_id == db.active_profile_id)
                .map(|p| format!("generated:{}", p.id)),
        )
        .collect::<Vec<_>>();
    pending_destructive.sort();

    WorkspaceSnapshot {
        mod_ids,
        enabled_mod_ids,
        mod_order,
        plugin_order: db.plugin_load_order.clone(),
        plugin_disabled: db.plugin_disabled.clone().into_iter().collect(),
        generated_outputs,
        pending_destructive,
    }
}

fn compute_pending_changes(
    baseline: Option<&WorkspaceBaseline>,
    db: &ModDatabase,
    runtime: &WorkspaceRuntimeState,
) -> PendingChanges {
    let empty_baseline = WorkspaceBaseline {
        profile_id: db.active_profile_id.clone(),
        snapshot: WorkspaceSnapshot::default(),
    };
    let baseline = baseline.unwrap_or(&empty_baseline);
    let now = snapshot_from_db(db);
    PendingChanges {
        mod_set_changed: now.mod_ids != baseline.snapshot.mod_ids,
        mod_enabled_changed: now.enabled_mod_ids != baseline.snapshot.enabled_mod_ids,
        mod_order_changed: now.mod_order != baseline.snapshot.mod_order,
        plugin_order_changed: now.plugin_order != baseline.snapshot.plugin_order
            || now.plugin_disabled != baseline.snapshot.plugin_disabled,
        generated_outputs_changed: now.generated_outputs != baseline.snapshot.generated_outputs,
        unmanaged_runtime_changed: runtime.unmanaged_runtime_changed,
        pending_destructive_changes: now.pending_destructive
            != baseline.snapshot.pending_destructive,
    }
}

pub fn mark_operation(game_id: &str, profile_id: &str, op: WorkspaceOperation) {
    let mut map = runtime_state_map().lock().expect("workspace lock poisoned");
    map.entry(runtime_key(game_id, profile_id))
        .or_default()
        .operation = op;
}

pub fn set_status(
    game_id: &str,
    profile_id: &str,
    severity: StatusSeverity,
    message: impl Into<String>,
) {
    let mut map = runtime_state_map().lock().expect("workspace lock poisoned");
    let state = map.entry(runtime_key(game_id, profile_id)).or_default();
    state.severity = severity;
    state.status_message = Some(message.into());
}

pub fn clear_status(game_id: &str, profile_id: &str) {
    let mut map = runtime_state_map().lock().expect("workspace lock poisoned");
    if let Some(state) = map.get_mut(&runtime_key(game_id, profile_id)) {
        state.status_message = None;
        state.severity = StatusSeverity::Info;
    }
}

pub fn mark_unmanaged_runtime_changes(game_id: &str, profile_id: &str, changed: bool) {
    let mut map = runtime_state_map().lock().expect("workspace lock poisoned");
    map.entry(runtime_key(game_id, profile_id))
        .or_default()
        .unmanaged_runtime_changed = changed;
}

pub fn update_runtime_scan_status(game_id: &str, profile_id: &str, report: &RuntimeScanReport) {
    let mut map = runtime_state_map().lock().expect("workspace lock poisoned");
    let state = map.entry(runtime_key(game_id, profile_id)).or_default();
    state.unmanaged_runtime_changed = report.has_unresolved_changes();
    state.runtime_scan = RuntimeScanStatus {
        unresolved_review_count: report.unresolved_review_count(),
        adoptable_count: report.adoptable_count(),
        unknown_count: report.unknown_count(),
    };
}

pub fn mark_deploy_failure(game_id: &str, profile_id: &str, message: impl Into<String>) {
    let mut map = runtime_state_map().lock().expect("workspace lock poisoned");
    let state = map.entry(runtime_key(game_id, profile_id)).or_default();
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
    let state = map
        .entry(runtime_key(&game.id, &db.active_profile_id))
        .or_default();
    state.deploy_failed = false;
    state.operation = WorkspaceOperation::None;
    state.unmanaged_runtime_changed = false;
    state.runtime_scan = RuntimeScanStatus::default();
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
        .get(&runtime_key(&game.id, &profile_id))
        .cloned()
        .unwrap_or_default();

    let pending_changes = compute_pending_changes(baseline.as_ref(), &db, &runtime);
    let runtime_review_required = runtime.runtime_scan.unresolved_review_count > 0;
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
        safe_redeploy_required: dirty || runtime.deploy_failed || runtime_review_required,
        safe_redeploy_recommended: dirty || runtime_review_required,
        runtime_review_required,
        runtime_adoptable_pending: runtime.runtime_scan.adoptable_count,
        runtime_unknown_pending: runtime.runtime_scan.unknown_count,
    }
}

fn deployment_state_label(state: DeploymentState) -> &'static str {
    match state {
        DeploymentState::Deployed => "Deployed",
        DeploymentState::NotDeployed => "Not deployed",
        DeploymentState::Dirty => "Redeploy needed",
        DeploymentState::Busy => "Deploying…",
        DeploymentState::Failed => "Deploy failed",
    }
}

pub fn format_workspace_banner_summary(state: &WorkspaceState) -> String {
    let mut summary = format!(
        "Profile: {} · {}",
        state.profile_id,
        deployment_state_label(state.deployment_state)
    );
    let reasons = state.pending_changes.reasons();
    if !reasons.is_empty() {
        summary.push_str(" · ");
        summary.push_str(&reasons.join(", "));
    }
    if let Some(msg) = state.status_message.as_deref()
        && !msg.trim().is_empty()
    {
        summary.push_str(" · ");
        summary.push_str(msg);
    }
    if state.runtime_review_required {
        summary.push_str(" · Review runtime changes first");
    }
    summary
}

pub fn format_workspace_compact_summary(state: &WorkspaceState) -> String {
    let reasons = state.pending_changes.reasons();
    if reasons.is_empty() {
        if state.runtime_review_required {
            format!(
                "{} · Runtime review pending (adoptable: {}, unknown: {})",
                deployment_state_label(state.deployment_state),
                state.runtime_adoptable_pending,
                state.runtime_unknown_pending
            )
        } else {
            format!("{} · clean", deployment_state_label(state.deployment_state))
        }
    } else {
        let mut summary = format!(
            "{} · {}",
            deployment_state_label(state.deployment_state),
            reasons.join(", ")
        );
        if state.runtime_review_required {
            summary.push_str(&format!(
                " · Runtime review pending (adoptable: {}, unknown: {})",
                state.runtime_adoptable_pending, state.runtime_unknown_pending
            ));
        }
        summary
    }
}

pub fn format_redeploy_guidance(
    state: &WorkspaceState,
    preview: Option<&crate::core::deployment::DeploymentPreview>,
) -> String {
    if state.current_operation == WorkspaceOperation::Deploy {
        return "Deploy in progress".to_string();
    }
    if state.deployment_state == DeploymentState::Failed {
        return "Deploy failed; review errors before retry".to_string();
    }
    if let Some(preview) = preview
        && !preview.blocked_paths.is_empty()
    {
        return format!(
            "Redeploy blocked by {} path issue(s)",
            preview.blocked_paths.len()
        );
    }
    if state.runtime_review_required {
        return format!(
            "Review runtime changes first (adoptable: {}, unknown: {})",
            state.runtime_adoptable_pending, state.runtime_unknown_pending
        );
    }
    if state.safe_redeploy_required {
        return "Redeploy ready after review".to_string();
    }
    if state.safe_redeploy_recommended {
        return "Redeploy recommended after review".to_string();
    }
    "No redeploy needed".to_string()
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

        mark_operation(&game.id, &db.active_profile_id, WorkspaceOperation::Deploy);
        let busy = workspace_state_for_game(&game);
        assert_eq!(busy.deployment_state, DeploymentState::Busy);

        mark_deploy_failure(&game.id, &db.active_profile_id, "deploy failed");
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
            runtime_review_required: false,
            runtime_adoptable_pending: 0,
            runtime_unknown_pending: 0,
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

        let state_allowed = WorkspaceState {
            safe_redeploy_required: false,
            ..state_dirty
        };
        assert!(matches!(
            profile_switch_policy(&state_allowed),
            ProfileSwitchPolicy::Allowed
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

    #[test]
    fn no_baseline_non_empty_profile_reports_pending_reasons() {
        let temp = TempDir::new().unwrap();
        let game = test_game(&temp, "workspace_no_baseline_pending");
        let mut db = ModDatabase::default();
        db.mods.push(crate::core::mods::Mod::new(
            "A",
            temp.path().join("mods").join("a"),
        ));
        db.mods[0].enabled = true;
        db.plugin_load_order.push("Patch.esp".to_string());
        db.generated_outputs
            .push(crate::core::mods::GeneratedOutputPackage::new(
                "Out",
                "tool",
                "default",
                db.active_profile_id.clone(),
                temp.path().join("gen"),
            ));
        db.save(&game);

        let state = workspace_state_for_game(&game);
        assert_eq!(state.deployment_state, DeploymentState::Dirty);
        assert!(state.pending_changes.mod_set_changed);
        assert!(state.pending_changes.mod_enabled_changed);
        assert!(state.pending_changes.mod_order_changed);
        assert!(state.pending_changes.plugin_order_changed);
        assert!(state.pending_changes.generated_outputs_changed);
        assert!(state.safe_redeploy_required);
    }

    #[test]
    fn runtime_state_is_profile_aware_and_does_not_leak() {
        let temp = TempDir::new().unwrap();
        let game = test_game(&temp, "workspace_runtime_profile_aware");
        let mut db = ModDatabase::default();
        db.save(&game);
        mark_deployed_clean(&game, &db).unwrap();

        mark_operation(&game.id, "profile_a", WorkspaceOperation::ToolRun);
        set_status(
            &game.id,
            "profile_a",
            StatusSeverity::Warning,
            "Tool running in A",
        );

        db.active_profile_id = "profile_b".to_string();
        db.save(&game);
        let state_b = workspace_state_for_game(&game);
        assert_eq!(state_b.current_operation, WorkspaceOperation::None);
        assert!(state_b.status_message.is_none());

        db.active_profile_id = "profile_a".to_string();
        db.save(&game);
        let state_a = workspace_state_for_game(&game);
        assert_eq!(state_a.current_operation, WorkspaceOperation::ToolRun);
        assert_eq!(state_a.status_message.as_deref(), Some("Tool running in A"));
    }

    #[test]
    fn generated_output_enable_disable_and_remove_affect_dirty_state() {
        let temp = TempDir::new().unwrap();
        let game = test_game(&temp, "workspace_output_toggle_remove");
        let mut db = ModDatabase::default();
        let mut pkg = crate::core::mods::GeneratedOutputPackage::new(
            "Out",
            "tool",
            "default",
            db.active_profile_id.clone(),
            temp.path().join("gen"),
        );
        pkg.enabled = true;
        db.generated_outputs.push(pkg);
        db.save(&game);
        mark_deployed_clean(&game, &db).unwrap();

        db.generated_outputs[0].enabled = false;
        db.save(&game);
        let disabled_state = workspace_state_for_game(&game);
        assert!(disabled_state.pending_changes.generated_outputs_changed);

        db.generated_outputs.clear();
        db.save(&game);
        let removed_state = workspace_state_for_game(&game);
        assert!(removed_state.pending_changes.generated_outputs_changed);
    }

    #[test]
    fn generated_outputs_are_profile_scoped_for_dirty_evaluation() {
        let temp = TempDir::new().unwrap();
        let game = test_game(&temp, "workspace_output_profile_scope");
        let mut db = ModDatabase::default();
        let pkg_a = crate::core::mods::GeneratedOutputPackage::new(
            "Out A",
            "tool_a",
            "default",
            "profile_a",
            temp.path().join("gen_a"),
        );
        let pkg_b = crate::core::mods::GeneratedOutputPackage::new(
            "Out B",
            "tool_b",
            "default",
            "profile_b",
            temp.path().join("gen_b"),
        );
        db.generated_outputs.push(pkg_a);
        db.generated_outputs.push(pkg_b);
        db.active_profile_id = "profile_a".to_string();
        db.save(&game);
        mark_deployed_clean(&game, &db).unwrap();

        db.active_profile_id = "profile_b".to_string();
        db.save(&game);
        let profile_b_state = workspace_state_for_game(&game);
        assert!(profile_b_state.pending_changes.generated_outputs_changed);
    }

    #[test]
    fn redeploy_summary_format_is_truthful() {
        let state = WorkspaceState {
            game_id: "g".to_string(),
            profile_id: "p".to_string(),
            deployment_state: DeploymentState::Dirty,
            pending_changes: PendingChanges {
                generated_outputs_changed: true,
                ..PendingChanges::default()
            },
            current_operation: WorkspaceOperation::None,
            status_message: Some("Outputs captured".to_string()),
            status_severity: StatusSeverity::Info,
            safe_redeploy_required: true,
            safe_redeploy_recommended: true,
            runtime_review_required: true,
            runtime_adoptable_pending: 1,
            runtime_unknown_pending: 0,
        };
        let banner = format_workspace_banner_summary(&state);
        let compact = format_workspace_compact_summary(&state);
        assert!(banner.contains("Profile: p"));
        assert!(banner.contains("Redeploy needed"));
        assert!(banner.contains("Generated outputs changed"));
        assert!(compact.contains("Redeploy needed"));
    }

    #[test]
    fn redeploy_guidance_prioritizes_blocked_and_runtime_review() {
        let state = WorkspaceState {
            game_id: "g".to_string(),
            profile_id: "p".to_string(),
            deployment_state: DeploymentState::Dirty,
            pending_changes: PendingChanges::default(),
            current_operation: WorkspaceOperation::None,
            status_message: None,
            status_severity: StatusSeverity::Info,
            safe_redeploy_required: true,
            safe_redeploy_recommended: true,
            runtime_review_required: true,
            runtime_adoptable_pending: 2,
            runtime_unknown_pending: 1,
        };
        let blocked_preview = crate::core::deployment::DeploymentPreview {
            blocked_paths: vec!["Data/textures".into()],
            ..crate::core::deployment::DeploymentPreview::default()
        };
        let blocked = format_redeploy_guidance(&state, Some(&blocked_preview));
        assert!(blocked.contains("blocked"));

        let review = format_redeploy_guidance(&state, None);
        assert!(review.contains("Review runtime changes first"));
    }

    #[test]
    fn unresolved_runtime_scan_updates_workspace_truth() {
        let temp = TempDir::new().unwrap();
        let game = test_game(&temp, "workspace_runtime_scan_truth");
        let db = ModDatabase::default();
        db.save(&game);
        let report = RuntimeScanReport {
            entries: vec![crate::core::runtime_scan::RuntimeScanEntry {
                relative_path: PathBuf::from("textures/runtime.dds"),
                classification:
                    crate::core::runtime_scan::RuntimeEntryClassification::UnmanagedAdoptable,
                review_status: crate::core::runtime_scan::RuntimeEntryReviewStatus::Pending,
                package_id: None,
                tool_id: None,
                explanation: "pending".to_string(),
            }],
        };
        update_runtime_scan_status(&game.id, &db.active_profile_id, &report);
        let state = workspace_state_for_game(&game);
        assert!(state.runtime_review_required);
        assert!(state.safe_redeploy_required);
        assert_eq!(state.runtime_adoptable_pending, 1);
    }
}
