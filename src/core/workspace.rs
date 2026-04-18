use crate::core::deployment::{self, DeploymentIntegrityLevel, DeploymentIntegrityReport};
use crate::core::games::Game;
use crate::core::mods::ModDatabase;
use crate::core::runtime_scan::RuntimeScanReport;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock, mpsc};

const WORKSPACE_BASELINE_FILE: &str = "workspace_baseline.toml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceEvent {
    ProfileStateChanged { game_id: String, profile_id: String },
    WorkspaceStateChanged { game_id: String, profile_id: String },
    DeployStarted { game_id: String, profile_id: String },
    DeployFinished { game_id: String, profile_id: String },
    DeployFailed { game_id: String, profile_id: String },
    ProfileSwitched { game_id: String, profile_id: String },
    RevertCompleted { game_id: String, profile_id: String },
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrityRepairOutlook {
    Healthy,
    RedeployLikelyRepairsAll,
    RedeployLikelyRepairsSome,
    ManualReviewRequired,
    Unknown,
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
    pub integrity_level: DeploymentIntegrityLevel,
    pub integrity_issue_total: usize,
    pub integrity_repairable_issues: usize,
    pub integrity_manual_review_issues: usize,
    pub integrity_summary: String,
    pub integrity_repair_outlook: IntegrityRepairOutlook,
    pub integrity_examples: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
struct WorkspaceSnapshot {
    mod_ids: Vec<String>,
    enabled_mod_ids: BTreeSet<String>,
    mod_order: Vec<String>,
    plugin_order: Vec<String>,
    plugin_disabled: BTreeSet<String>,
    generated_outputs: Vec<String>,
    #[serde(default)]
    generated_output_enabled: BTreeMap<String, bool>,
    #[serde(default)]
    pending_destructive: Vec<String>,
    #[serde(default)]
    runtime_ignored: BTreeSet<String>,
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
    integrity_report: Option<DeploymentIntegrityReport>,
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
            integrity_report: None,
        }
    }
}

static RUNTIME_STATE: OnceLock<Mutex<HashMap<WorkspaceRuntimeKey, WorkspaceRuntimeState>>> =
    OnceLock::new();
static WORKSPACE_SUBSCRIBERS: OnceLock<Mutex<Vec<mpsc::Sender<WorkspaceEvent>>>> = OnceLock::new();

fn runtime_state_map() -> &'static Mutex<HashMap<WorkspaceRuntimeKey, WorkspaceRuntimeState>> {
    RUNTIME_STATE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn subscriber_list() -> &'static Mutex<Vec<mpsc::Sender<WorkspaceEvent>>> {
    WORKSPACE_SUBSCRIBERS.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn subscribe_events() -> mpsc::Receiver<WorkspaceEvent> {
    let (tx, rx) = mpsc::channel();
    subscriber_list()
        .lock()
        .expect("workspace subscribers lock poisoned")
        .push(tx);
    rx
}

pub fn emit_event(event: WorkspaceEvent) {
    let mut subscribers = subscriber_list()
        .lock()
        .expect("workspace subscribers lock poisoned");
    subscribers.retain(|tx| tx.send(event.clone()).is_ok());
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
    let generated_output_enabled = db
        .generated_outputs
        .iter()
        .filter(|o| o.manager_profile_id == db.active_profile_id)
        .map(|o| (o.id.clone(), o.enabled))
        .collect::<BTreeMap<_, _>>();
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
    let runtime_ignored = db
        .profile_runtime_ignored
        .get(&db.active_profile_id)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect();

    WorkspaceSnapshot {
        mod_ids,
        enabled_mod_ids,
        mod_order,
        plugin_order: db.plugin_load_order.clone(),
        plugin_disabled: db.plugin_disabled.clone().into_iter().collect(),
        generated_outputs,
        generated_output_enabled,
        pending_destructive,
        runtime_ignored,
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
    emit_event(WorkspaceEvent::WorkspaceStateChanged {
        game_id: game_id.to_string(),
        profile_id: profile_id.to_string(),
    });
    if op == WorkspaceOperation::Deploy {
        emit_event(WorkspaceEvent::DeployStarted {
            game_id: game_id.to_string(),
            profile_id: profile_id.to_string(),
        });
    }
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
    emit_event(WorkspaceEvent::WorkspaceStateChanged {
        game_id: game_id.to_string(),
        profile_id: profile_id.to_string(),
    });
}

pub fn clear_status(game_id: &str, profile_id: &str) {
    let mut map = runtime_state_map().lock().expect("workspace lock poisoned");
    if let Some(state) = map.get_mut(&runtime_key(game_id, profile_id)) {
        state.status_message = None;
        state.severity = StatusSeverity::Info;
    }
    emit_event(WorkspaceEvent::WorkspaceStateChanged {
        game_id: game_id.to_string(),
        profile_id: profile_id.to_string(),
    });
}

pub fn mark_unmanaged_runtime_changes(game_id: &str, profile_id: &str, changed: bool) {
    let mut map = runtime_state_map().lock().expect("workspace lock poisoned");
    map.entry(runtime_key(game_id, profile_id))
        .or_default()
        .unmanaged_runtime_changed = changed;
    emit_event(WorkspaceEvent::WorkspaceStateChanged {
        game_id: game_id.to_string(),
        profile_id: profile_id.to_string(),
    });
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
    emit_event(WorkspaceEvent::WorkspaceStateChanged {
        game_id: game_id.to_string(),
        profile_id: profile_id.to_string(),
    });
}

pub fn latest_integrity_report(
    game_id: &str,
    profile_id: &str,
) -> Option<DeploymentIntegrityReport> {
    runtime_state_map()
        .lock()
        .expect("workspace lock poisoned")
        .get(&runtime_key(game_id, profile_id))
        .and_then(|state| state.integrity_report.clone())
}

pub fn verify_and_store_integrity(game: &Game) -> Result<DeploymentIntegrityReport, String> {
    let db = ModDatabase::load(game);
    let profile_id = db.active_profile_id.clone();
    let report = deployment::verify_deployment_integrity(game, &db)?;
    {
        let mut map = runtime_state_map().lock().expect("workspace lock poisoned");
        let state = map.entry(runtime_key(&game.id, &profile_id)).or_default();
        state.integrity_report = Some(report.clone());
    }
    emit_event(WorkspaceEvent::WorkspaceStateChanged {
        game_id: game.id.clone(),
        profile_id,
    });
    Ok(report)
}

pub fn mark_deploy_failure(game_id: &str, profile_id: &str, message: impl Into<String>) {
    let mut map = runtime_state_map().lock().expect("workspace lock poisoned");
    let state = map.entry(runtime_key(game_id, profile_id)).or_default();
    state.deploy_failed = true;
    state.severity = StatusSeverity::Error;
    state.status_message = Some(message.into());
    state.operation = WorkspaceOperation::None;
    emit_event(WorkspaceEvent::WorkspaceStateChanged {
        game_id: game_id.to_string(),
        profile_id: profile_id.to_string(),
    });
    emit_event(WorkspaceEvent::DeployFailed {
        game_id: game_id.to_string(),
        profile_id: profile_id.to_string(),
    });
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
    state.integrity_report = None;
    state.status_message = Some("Deployment is up to date".to_string());
    state.severity = StatusSeverity::Info;
    emit_event(WorkspaceEvent::WorkspaceStateChanged {
        game_id: game.id.clone(),
        profile_id: db.active_profile_id.clone(),
    });
    emit_event(WorkspaceEvent::DeployFinished {
        game_id: game.id.clone(),
        profile_id: db.active_profile_id.clone(),
    });
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
    let integrity_report = runtime.integrity_report.as_ref();
    let integrity_level = integrity_report
        .and_then(|r| r.level)
        .unwrap_or(DeploymentIntegrityLevel::Unknown);
    let integrity_issue_total = integrity_report.map(|r| r.issue_count()).unwrap_or(0);
    let integrity_repairable_issues = integrity_report.map(|r| r.repairable_count()).unwrap_or(0);
    let integrity_manual_review_issues = integrity_report
        .map(|r| r.manual_review_count())
        .unwrap_or(0);
    let integrity_summary = integrity_report
        .map(|r| r.summary_line())
        .unwrap_or_else(|| "Integrity: unknown (not verified)".to_string());
    let integrity_examples = integrity_report
        .map(|r| r.top_examples(3))
        .unwrap_or_default();
    let integrity_repair_outlook = if integrity_issue_total == 0 {
        if integrity_level == DeploymentIntegrityLevel::Healthy {
            IntegrityRepairOutlook::Healthy
        } else {
            IntegrityRepairOutlook::Unknown
        }
    } else if integrity_manual_review_issues == 0 {
        IntegrityRepairOutlook::RedeployLikelyRepairsAll
    } else if integrity_repairable_issues == 0 {
        IntegrityRepairOutlook::ManualReviewRequired
    } else {
        IntegrityRepairOutlook::RedeployLikelyRepairsSome
    };
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
        safe_redeploy_required: dirty
            || runtime.deploy_failed
            || runtime_review_required
            || integrity_issue_total > 0,
        safe_redeploy_recommended: dirty || runtime_review_required || integrity_issue_total > 0,
        runtime_review_required,
        runtime_adoptable_pending: runtime.runtime_scan.adoptable_count,
        runtime_unknown_pending: runtime.runtime_scan.unknown_count,
        integrity_level,
        integrity_issue_total,
        integrity_repairable_issues,
        integrity_manual_review_issues,
        integrity_summary,
        integrity_repair_outlook,
        integrity_examples,
    }
}

pub fn has_profile_baseline(game: &Game, profile_id: &str) -> bool {
    load_baseline(game, profile_id).is_some()
}

pub fn notify_profile_state_changed(game_id: &str, profile_id: &str) {
    emit_event(WorkspaceEvent::ProfileStateChanged {
        game_id: game_id.to_string(),
        profile_id: profile_id.to_string(),
    });
    emit_event(WorkspaceEvent::WorkspaceStateChanged {
        game_id: game_id.to_string(),
        profile_id: profile_id.to_string(),
    });
}

pub fn revert_active_profile_to_baseline(game: &Game) -> Result<(), String> {
    let mut db = ModDatabase::load(game);
    let profile_id = db.active_profile_id.clone();
    let Some(baseline) = load_baseline(game, &profile_id) else {
        return Err("No deployed baseline exists for this profile yet".to_string());
    };
    let snapshot = baseline.snapshot;
    let mod_ids_in_baseline: HashSet<String> = snapshot.mod_ids.iter().cloned().collect();
    let pending_destructive: HashSet<String> = snapshot.pending_destructive.into_iter().collect();

    for m in &mut db.mods {
        m.enabled = snapshot.enabled_mod_ids.contains(&m.id);
        m.pending_removal = pending_destructive.contains(&format!("mod:{}", m.id));
        if !mod_ids_in_baseline.contains(&m.id) {
            m.enabled = false;
            m.pending_removal = true;
        }
    }

    let mut mod_order = Vec::new();
    for id in &snapshot.mod_order {
        if let Some(entry) = db.mods.iter().find(|m| &m.id == id).cloned() {
            mod_order.push(entry);
        }
    }
    for entry in &db.mods {
        if !mod_order.iter().any(|m| m.id == entry.id) {
            mod_order.push(entry.clone());
        }
    }
    db.mods = mod_order;

    db.plugin_load_order = snapshot.plugin_order;
    db.plugin_disabled = snapshot.plugin_disabled.into_iter().collect();

    for pkg in &mut db.generated_outputs {
        if pkg.manager_profile_id != profile_id {
            continue;
        }
        pkg.enabled = snapshot
            .generated_output_enabled
            .get(&pkg.id)
            .copied()
            .unwrap_or(false);
        pkg.pending_removal = pending_destructive.contains(&format!("generated:{}", pkg.id));
        if !snapshot.generated_output_enabled.contains_key(&pkg.id) {
            pkg.enabled = false;
            pkg.pending_removal = true;
        }
    }

    db.profile_runtime_ignored.insert(
        profile_id.clone(),
        snapshot.runtime_ignored.into_iter().collect(),
    );
    db.save(game);
    set_status(
        &game.id,
        &profile_id,
        StatusSeverity::Info,
        "Discarded staged edits; profile state restored to deployed baseline",
    );
    emit_event(WorkspaceEvent::RevertCompleted {
        game_id: game.id.clone(),
        profile_id,
    });
    Ok(())
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

fn integrity_compact_label(state: &WorkspaceState) -> String {
    match state.integrity_level {
        DeploymentIntegrityLevel::Healthy => "Integrity healthy".to_string(),
        DeploymentIntegrityLevel::Unknown => "Integrity unknown".to_string(),
        DeploymentIntegrityLevel::Warnings => format!(
            "Integrity warnings: {} issue(s), {} repairable by redeploy",
            state.integrity_issue_total, state.integrity_repairable_issues
        ),
        DeploymentIntegrityLevel::Broken => format!(
            "Integrity broken: {} issue(s), {} manual review",
            state.integrity_issue_total, state.integrity_manual_review_issues
        ),
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
    summary.push_str(" · ");
    summary.push_str(&integrity_compact_label(state));
    summary
}

pub fn format_workspace_compact_summary(state: &WorkspaceState) -> String {
    let reasons = state.pending_changes.reasons();
    if reasons.is_empty() {
        if state.runtime_review_required {
            format!(
                "{} · Runtime review pending (adoptable: {}, unknown: {}) · {}",
                deployment_state_label(state.deployment_state),
                state.runtime_adoptable_pending,
                state.runtime_unknown_pending,
                integrity_compact_label(state)
            )
        } else {
            format!(
                "{} · clean · {}",
                deployment_state_label(state.deployment_state),
                integrity_compact_label(state)
            )
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
        summary.push_str(" · ");
        summary.push_str(&integrity_compact_label(state));
        summary
    }
}

pub fn format_integrity_guidance(state: &WorkspaceState) -> String {
    if state.integrity_issue_total == 0 {
        return match state.integrity_level {
            DeploymentIntegrityLevel::Healthy => "Deployed integrity is healthy".to_string(),
            _ => "Deployed integrity has not been verified yet".to_string(),
        };
    }
    match state.integrity_repair_outlook {
        IntegrityRepairOutlook::RedeployLikelyRepairsAll => format!(
            "Integrity drift detected: redeploy is likely to repair all {} issue(s)",
            state.integrity_issue_total
        ),
        IntegrityRepairOutlook::RedeployLikelyRepairsSome => format!(
            "Integrity drift detected: redeploy may repair {} issue(s), but {} still need manual review",
            state.integrity_repairable_issues, state.integrity_manual_review_issues
        ),
        IntegrityRepairOutlook::ManualReviewRequired => format!(
            "Integrity drift detected: {} issue(s) require manual review before full recovery",
            state.integrity_manual_review_issues
        ),
        IntegrityRepairOutlook::Healthy | IntegrityRepairOutlook::Unknown => {
            "Deployed integrity state is unknown; run verification".to_string()
        }
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
        if state.integrity_issue_total > 0 {
            return "Deploy failed and integrity drift is present; review issues before retry"
                .to_string();
        }
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
    if state.integrity_issue_total > 0 {
        return format_integrity_guidance(state);
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
            integrity_level: DeploymentIntegrityLevel::Unknown,
            integrity_issue_total: 0,
            integrity_repairable_issues: 0,
            integrity_manual_review_issues: 0,
            integrity_summary: "Integrity: unknown (not verified)".to_string(),
            integrity_repair_outlook: IntegrityRepairOutlook::Unknown,
            integrity_examples: Vec::new(),
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
            integrity_level: DeploymentIntegrityLevel::Broken,
            integrity_issue_total: 2,
            integrity_repairable_issues: 1,
            integrity_manual_review_issues: 1,
            integrity_summary: "Integrity: 2 issue(s), 1 require manual review".to_string(),
            integrity_repair_outlook: IntegrityRepairOutlook::RedeployLikelyRepairsSome,
            integrity_examples: vec!["Managed source path is missing".to_string()],
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
            integrity_level: DeploymentIntegrityLevel::Warnings,
            integrity_issue_total: 1,
            integrity_repairable_issues: 1,
            integrity_manual_review_issues: 0,
            integrity_summary: "Integrity: 1 issue(s), 1 likely repairable by redeploy".to_string(),
            integrity_repair_outlook: IntegrityRepairOutlook::RedeployLikelyRepairsAll,
            integrity_examples: vec!["Expected managed deployed path is missing".to_string()],
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
    fn integrity_report_maps_into_workspace_state() {
        let temp = TempDir::new().unwrap();
        let game = test_game(&temp, "workspace_integrity_truth");
        let db = ModDatabase::default();
        db.save(&game);

        {
            let mut map = runtime_state_map().lock().expect("workspace lock poisoned");
            map.entry(runtime_key(&game.id, &db.active_profile_id))
                .or_default()
                .integrity_report = Some(DeploymentIntegrityReport {
                level: Some(DeploymentIntegrityLevel::Broken),
                issues: vec![
                    crate::core::deployment::DeploymentIntegrityIssue {
                        kind: crate::core::deployment::DeploymentIntegrityIssueKind::ManagedSourceMissing,
                        path: Some(PathBuf::from("/tmp/missing")),
                        details: "Managed source path is missing".to_string(),
                        repairable_by_redeploy: false,
                    },
                    crate::core::deployment::DeploymentIntegrityIssue {
                        kind: crate::core::deployment::DeploymentIntegrityIssueKind::ExpectedTargetMissing,
                        path: Some(PathBuf::from("/tmp/dest")),
                        details: "Expected managed deployed path is missing".to_string(),
                        repairable_by_redeploy: true,
                    },
                ],
            });
        }

        let state = workspace_state_for_game(&game);
        assert_eq!(state.integrity_level, DeploymentIntegrityLevel::Broken);
        assert_eq!(state.integrity_issue_total, 2);
        assert_eq!(state.integrity_repairable_issues, 1);
        assert_eq!(state.integrity_manual_review_issues, 1);
        assert_eq!(
            state.integrity_repair_outlook,
            IntegrityRepairOutlook::RedeployLikelyRepairsSome
        );
        assert!(state.safe_redeploy_required);
    }

    #[test]
    fn redeploy_guidance_mentions_failed_plus_integrity() {
        let state = WorkspaceState {
            game_id: "g".to_string(),
            profile_id: "p".to_string(),
            deployment_state: DeploymentState::Failed,
            pending_changes: PendingChanges::default(),
            current_operation: WorkspaceOperation::None,
            status_message: None,
            status_severity: StatusSeverity::Error,
            safe_redeploy_required: true,
            safe_redeploy_recommended: true,
            runtime_review_required: false,
            runtime_adoptable_pending: 0,
            runtime_unknown_pending: 0,
            integrity_level: DeploymentIntegrityLevel::Broken,
            integrity_issue_total: 1,
            integrity_repairable_issues: 0,
            integrity_manual_review_issues: 1,
            integrity_summary: "Integrity: 1 issue(s), 1 require manual review".to_string(),
            integrity_repair_outlook: IntegrityRepairOutlook::ManualReviewRequired,
            integrity_examples: vec!["Managed source path is missing".to_string()],
        };
        let guidance = format_redeploy_guidance(&state, None);
        assert!(guidance.contains("Deploy failed and integrity drift is present"));
    }

    #[test]
    fn integrity_guidance_formats_repairability_truthfully() {
        let mut state = WorkspaceState {
            game_id: "g".to_string(),
            profile_id: "p".to_string(),
            deployment_state: DeploymentState::Dirty,
            pending_changes: PendingChanges::default(),
            current_operation: WorkspaceOperation::None,
            status_message: None,
            status_severity: StatusSeverity::Info,
            safe_redeploy_required: true,
            safe_redeploy_recommended: true,
            runtime_review_required: false,
            runtime_adoptable_pending: 0,
            runtime_unknown_pending: 0,
            integrity_level: DeploymentIntegrityLevel::Warnings,
            integrity_issue_total: 3,
            integrity_repairable_issues: 3,
            integrity_manual_review_issues: 0,
            integrity_summary: String::new(),
            integrity_repair_outlook: IntegrityRepairOutlook::RedeployLikelyRepairsAll,
            integrity_examples: Vec::new(),
        };
        assert!(format_integrity_guidance(&state).contains("repair all 3 issue(s)"));

        state.integrity_repair_outlook = IntegrityRepairOutlook::ManualReviewRequired;
        state.integrity_repairable_issues = 0;
        state.integrity_manual_review_issues = 3;
        assert!(format_integrity_guidance(&state).contains("require manual review"));
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

    #[test]
    fn revert_restores_staged_mod_plugin_and_pending_state_to_baseline() {
        let temp = TempDir::new().unwrap();
        let game = test_game(&temp, "workspace_revert_baseline");
        let mut db = ModDatabase::default();
        let mut m1 = crate::core::mods::Mod::new("M1", temp.path().join("m1"));
        let mut m2 = crate::core::mods::Mod::new("M2", temp.path().join("m2"));
        m1.enabled = true;
        m2.enabled = false;
        db.mods = vec![m1.clone(), m2.clone()];
        db.plugin_load_order = vec!["A.esp".to_string(), "B.esp".to_string()];
        db.plugin_disabled = ["B.esp".to_string()].into_iter().collect();
        let mut pkg = crate::core::mods::GeneratedOutputPackage::new(
            "Out",
            "tool",
            "default",
            db.active_profile_id.clone(),
            temp.path().join("out"),
        );
        pkg.enabled = true;
        db.generated_outputs.push(pkg.clone());
        db.profile_runtime_ignored.insert(
            db.active_profile_id.clone(),
            ["Data/ignore.txt".to_string()].into_iter().collect(),
        );
        db.save(&game);
        mark_deployed_clean(&game, &db).unwrap();

        let mut staged = ModDatabase::load(&game);
        staged.mods.swap(0, 1);
        staged.mods[0].enabled = true;
        staged.mods[1].enabled = false;
        staged.mods[1].pending_removal = true;
        let mut extra = crate::core::mods::Mod::new("Extra", temp.path().join("m3"));
        extra.enabled = true;
        staged.mods.push(extra.clone());
        staged.plugin_load_order = vec!["B.esp".to_string(), "A.esp".to_string()];
        staged.plugin_disabled.clear();
        staged.generated_outputs[0].enabled = false;
        staged.generated_outputs[0].pending_removal = true;
        staged.profile_runtime_ignored.insert(
            staged.active_profile_id.clone(),
            ["Data/other.txt".to_string()].into_iter().collect(),
        );
        staged.save(&game);

        revert_active_profile_to_baseline(&game).unwrap();
        let reverted = ModDatabase::load(&game);
        assert_eq!(reverted.mods[0].id, m1.id);
        assert_eq!(reverted.mods[1].id, m2.id);
        assert!(reverted.mods[0].enabled);
        assert!(!reverted.mods[1].enabled);
        assert!(!reverted.mods[0].pending_removal);
        assert!(!reverted.mods[1].pending_removal);
        let extra_reverted = reverted.mods.iter().find(|m| m.id == extra.id).unwrap();
        assert!(extra_reverted.pending_removal);
        assert!(!extra_reverted.enabled);
        assert_eq!(
            reverted.plugin_load_order,
            vec!["A.esp".to_string(), "B.esp".to_string()]
        );
        assert!(reverted.plugin_disabled.contains("B.esp"));
        assert!(reverted.generated_outputs[0].enabled);
        assert!(!reverted.generated_outputs[0].pending_removal);
        let ignored = reverted
            .profile_runtime_ignored
            .get(&reverted.active_profile_id)
            .cloned()
            .unwrap_or_default();
        assert!(ignored.contains("Data/ignore.txt"));
        assert!(!ignored.contains("Data/other.txt"));
    }

    #[test]
    fn revert_without_baseline_is_rejected() {
        let temp = TempDir::new().unwrap();
        let game = test_game(&temp, "workspace_revert_no_baseline");
        let db = ModDatabase::default();
        db.save(&game);
        let err = revert_active_profile_to_baseline(&game).unwrap_err();
        assert!(err.contains("No deployed baseline"));
    }

    #[test]
    fn workspace_event_subscribers_receive_notifications() {
        let rx = subscribe_events();
        emit_event(WorkspaceEvent::DeployStarted {
            game_id: "g".to_string(),
            profile_id: "p".to_string(),
        });
        let ev = rx
            .recv_timeout(std::time::Duration::from_millis(100))
            .unwrap();
        assert_eq!(
            ev,
            WorkspaceEvent::DeployStarted {
                game_id: "g".to_string(),
                profile_id: "p".to_string(),
            }
        );
    }

    #[test]
    fn notify_profile_state_changed_emits_profile_and_workspace_events() {
        let rx = subscribe_events();
        notify_profile_state_changed("g2", "p2");
        let first = rx
            .recv_timeout(std::time::Duration::from_millis(100))
            .unwrap();
        let second = rx
            .recv_timeout(std::time::Duration::from_millis(100))
            .unwrap();
        assert!(matches!(
            first,
            WorkspaceEvent::ProfileStateChanged { .. }
                | WorkspaceEvent::WorkspaceStateChanged { .. }
        ));
        assert!(matches!(
            second,
            WorkspaceEvent::ProfileStateChanged { .. }
                | WorkspaceEvent::WorkspaceStateChanged { .. }
        ));
    }
}
