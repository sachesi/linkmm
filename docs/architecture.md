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
