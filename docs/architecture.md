# Architecture

## 1. Deterministic rebuild engine

Deployment is computed from state and applied by deployers:

- `AssetsDeployer` for file payload
- `PluginsDeployer` for `plugins.txt`

No per-action imperative deploy/undeploy flow is the source of truth.

## 2. Profile-scoped state

`ModDatabase` stores active profile plus profile maps for:

- mod enabled/order
- plugin order/disabled
- generated output package lists

Deployment state (`deployment_state.toml`) is also profile-scoped under game config profile paths.

## 3. Generated output packages

Generated files from external tools are tracked as packages with owned-file metadata.

Sources:

- explicit output dir registration
- snapshot/diff capture
- unmanaged adoption from `Data/`

## 4. Tool orchestration

`tool_runs` orchestrates:

- adapter selection
- preflight validation
- execution
- capture/import
- rebuild

Adapters (`tool_adapters`) hold tool-specific policy:

- defaults
- validation
- output classification
- unmanaged detection heuristics

## 5. Game kind vs game instance

Game entries are instance-based, not kind-keyed:

- each managed game has a stable unique `id`
- `GameKind` defines family/type only (Skyrim SE, Fallout 4, etc.)
- multiple instances of the same `GameKind` are valid and supported

Examples:

- Skyrim SE (Steam)
- Skyrim SE (Non-Steam / UMU)
- Skyrim SE (Non-Steam / UMU, custom prefix)

## 6. Launcher-source contract

Each game instance has explicit `launcher_source`:

- `Steam`
- `NonSteamUmu`

This source drives:

- game launch backend
- tool launch backend
- `%LOCALAPPDATA%`/`plugins.txt` resolution
- per-instance launcher preferences in UI

No launch behavior is inferred from optional fields alone.

## 7. Managed runtime sessions (UMU/direct only)

Only directly-owned launches run through `core::runtime::RuntimeSessionManager`:

- Non-Steam UMU game launches
- Non-Steam UMU tool launches

The manager is the single source of truth for launch state and stores:

- stable session id
- session kind (`Game` or `Tool`)
- game instance id
- profile id (when known)
- tool id (tool sessions)
- launcher source (`Steam` / `NonSteamUmu`)
- tracked pid
- start time
- status (`Starting`, `Running`, `Exited`, `Failed`, `Killed`)
- exit code

It also owns per-session rolling log buffers for stdout/stderr capture.

## 8. Stop semantics

- Tool sessions are launched as tracked child processes and can be stopped from LinkMM.
- Non-Steam UMU game sessions are launched as tracked child processes and can be stopped from LinkMM.
- Steam game sessions are launch-only (not runtime-owned) with backend-specific launch commands:
  - native Steam installs: `steam -applaunch <app_id>`
  - Flatpak Steam installs: `flatpak run com.valvesoftware.Steam -applaunch <app_id>`
- Steam Stop semantics are intentionally not exposed as exact process ownership in LinkMM runtime.

## 9. UI state ownership and page lifecycle

LinkMM UI now treats navigation pages as long-lived widgets instead of recreating
them on every tab switch.

- Main navigation uses a stable `gtk4::Stack` with cached page instances.
- Tab switching changes visible child only; it must not rebuild full page trees.
- Expensive refreshes (scan/reload) are explicit page actions, not implicit side effects of navigation.

State ownership contract:

- **App-global/shared state**
  - long-running operation status (install busy / navigation lock)
  - whether navigation/game switching actions are allowed
  - global status surfaces that remain truthful regardless of current tab
- **Page-local state**
  - search text, scroll position, local selection/focus for list interactions
  - transient row-level controls and expansion/selection context

Long-running operations (install/deploy/runtime) must never rely solely on an
ephemeral page instance. If a status is critical for safety or user trust, it
must be represented in shared state and rendered consistently across navigation.

List/view update rule for Library and Load Order:

- Prefer targeted state updates where possible.
- If a list rebuild is required, preserve user context (scroll/focus/search) and
  avoid unrelated resets/jumping.
- Reordering actions operate on authoritative full-order lists only; when search
  filtering is active, reorder affordances are disabled until the filter is
  cleared.

## 10. Workspace state and dirty semantics

LinkMM now exposes an app-shared workspace state model per game instance/profile.

`core::workspace` is responsible for:

- current `game_id` and active `profile_id`
- deployment state (`Deployed`, `NotDeployed`, `Dirty`, `Busy`, `Failed`)
- current long-running operation (`None`, `Install`, `Deploy`, `ToolRun`, `Capture`, `Restore`)
- status message + severity
- pending change flags used for safe redeploy decisions

Dirty state is computed against a persisted per-profile baseline snapshot written
after successful deployment (`workspace_baseline.toml` under profile config).
When no baseline exists yet, the active profile is compared against an implicit
empty baseline so first-deploy state still reports truthful pending reasons.

Tracked dirty sources:

- mod set changed (install/uninstall)
- mod enabled/disabled changed
- mod order changed
- plugin order/disabled state changed
- generated output package set changed
- unmanaged/runtime changes explicitly flagged by tool/runtime flows

Safety contract:

- deployment success writes a fresh clean baseline and clears deploy-failed state
- deployment failure preserves truthful failed state and status message
- transient operation/status runtime state is profile-aware (keyed by game +
  profile) so profile A runtime state is never shown on profile B
- profile switching consults workspace policy (`Allowed` / `Warn` / `Blocked`);
  `Warn` requires explicit user confirmation while `Blocked` is denied

## 11. Tool runs and generated output lifecycle in workspace flow

Tool execution remains adapter-driven (`tool_runs` + `tool_adapters`) but now
feeds workspace state:

- launch marks operation as `ToolRun`
- success/failure updates shared status
- generated output import/adoption changes become dirty sources
- UI surfaces show whether capture/import happened and whether redeploy is needed

This keeps deterministic deploy as the source of truth while making tools and
runtime-preserved output part of one coherent profile workflow instead of an
isolated page action.

Generated outputs and runtime-preserved changes are exposed as a first-class
review/manage surface in Tools:

- redeploy guidance card driven directly by workspace state (`required` /
  `recommended` / clean)
- runtime-preserved change flag explanation for the active profile
- per-package metadata (tool, run profile, manager profile, update time, file count)
- per-package actions: enable/disable, reveal source directory, remove package

Output actions are deterministic and profile-aware:

- package enable/disable/remove updates `ModDatabase`
- deployment rebuild is triggered through the existing deterministic rebuild path
- workspace dirty reasons are derived from baseline-vs-current snapshots and
  therefore automatically reflect output state changes

## 12. Runtime/unmanaged scan and review model

LinkMM now has a scoped, profile-aware runtime scan model (`core::runtime_scan`)
for reviewable unmanaged/runtime changes in game `Data/` (and generated output
ownership expectations), rather than a coarse flag only.

Scan categories:

- `ManagedOwnedPresent`
- `ManagedOwnedMissing`
- `ManagedOwnedModified`
- `UnmanagedAdoptable`
- `UnmanagedIgnorable`
- `UnknownNeedsReview`

Each scan entry contains:

- relative path
- classification
- review status (`Pending`/`Ignored`)
- optional package/tool linkage when known
- short explanation text

Current scan scope is intentionally narrow and truthful:

- generated output package owned files for the active manager profile
- game `Data/` files in managed workflow areas already used for deploy/output
- no whole-prefix or random large-area indexing

Workflow integration:

- Tools “Outputs & Runtime Changes” supports manual rescan, per-entry adopt,
  ignore, reveal, and (for unmanaged files) explicit remove actions.
- Adoption uses explicit generated-output package creation/update flow and keeps
  behavior deterministic/profile-scoped.
- Scan summaries feed workspace runtime-review state so redeploy guidance can
  distinguish “redeploy now” vs “review runtime items first.”

## 13. Deployment backup payload storage and restore truth

Deployment keeps metadata/state in config (for example `deployment_state.toml`
under per-profile config), but backup payload files are now stored in manager-
owned profile data paths:

- backup payload root: `<mods_dir>/profiles/<profile>/deployment_backups/`
- config keeps backup mappings/ownership metadata only

Restore semantics are unchanged:

- if deployment must replace a real game file, LinkMM moves that original file
  into profile backup storage
- once no managed deployment entry owns that destination, LinkMM restores the
  original file from backup storage
- cross-filesystem move fallback (copy + remove) remains in place

## 14. Deployment preview planning and review contract

`core::deployment::deployment_preview` now computes a real dry-run plan for the
active game/profile without mutating filesystem state.

Planning uses the same authoritative deployment intent as real deploy:

- desired file ownership map from enabled mods + enabled generated outputs for
  the active profile (`assets` deployer plan)
- persisted per-profile deployment state (`deployment_state.toml`)
- current destination filesystem state

The planner emits `DeploymentPreview` with truthful consequences:

- links to create / replace / remove
- real files that will be backed up before link placement
- preserved originals that will be restored this deploy
- preserved backups that remain after deploy
- generated outputs participating in this deploy intent
- blocked paths that would prevent safe apply

Blocked path semantics:

- destination exists as an unsupported object (for example directory/device)
- restore destination cannot be safely replaced
- parent path conflicts (parent exists and is not a directory)
- path inspection failures relevant to deploy safety

Preview/apply drift prevention:

- both preview and apply use the shared assets execution planner
- apply refuses to start when planner reports blocked paths
- apply executes the same planned remove/apply/restore phases represented by
  preview output, so the UI review step reflects real deploy behavior

UI integration:

- Tools → Outputs & Runtime Changes shows redeploy guidance derived from
  workspace truth plus preview blocked-path status
- a redeploy preview row shows compact `summary_line()`
- grouped rows summarize create/replace/remove/backup/restore/blocked sets and
  generated outputs participation with concise examples

## 15. Staged profile edits vs explicit apply-deploy

Profile editing and deployment application are now separated:

- profile edits (enable/disable/reorder/toggle output/queue removal) update
  `ModDatabase` and workspace dirty truth only
- filesystem mutation happens only on explicit redeploy/apply actions
  (`rebuild_deployment` / `ModManager::rebuild_all`)

This keeps deterministic rebuild semantics while removing scattered implicit
apply side effects from normal editing workflows.

### Deferred destructive cleanup

Destructive edits are staged first and finalized only after successful redeploy:

- mods and generated output packages can be marked `pending_removal`
- pending removals are excluded from desired deploy intent immediately
- payload deletion and DB record removal happen in `finalize_pending_removals`
  after a successful deploy plan apply
- failed deploys do not finalize pending removals

This prevents broken deployed links caused by deleting source payloads before a
redeploy has switched filesystem state safely.

### Preview visibility for staged destructive changes

Deployment preview now includes pending destructive cleanup context:

- pending mod removals
- pending generated output removals
- payload paths that will be deleted after a successful redeploy

Tools surfaces this in the review card so users can inspect staged edits and
cleanup consequences before explicitly applying deployment.

Backup hygiene:

- restored payload files are removed from backup storage
- empty backup directories are pruned where practical
- stale unreferenced backup payload files can be cleaned safely without touching
  referenced backups

UI/Workspace surfacing:

- Outputs & Runtime Changes now includes a deployment backup status row
  (backup entry count, payload file count)
- users can reveal backup directory and run safe stale-backup cleanup
- redeploy status messaging includes whether preserved originals currently exist
