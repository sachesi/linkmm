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

## 7. Managed runtime sessions

Game launches and tool launches now run through `core::runtime::RuntimeSessionManager`.

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
Steam sessions can be either:

- direct child managed (`Running`) when LinkMM owns a stable process handle
- delegated (`DelegatedRunning`) when Steam handoff succeeds but the launcher wrapper is short-lived

## 8. Stop semantics

- Tool sessions are launched as tracked child processes and can be stopped from LinkMM.
- Non-Steam UMU game sessions are launched as tracked child processes and can be stopped from LinkMM.
- Steam game sessions use backend-specific managed commands:
  - native Steam installs: `steam -applaunch <app_id>`
  - Flatpak Steam installs: `flatpak run com.valvesoftware.Steam -applaunch <app_id>`
- Stop targets the spawned Steam wrapper process. Steam can re-parent the real game process, so
  stop/kill visibility for both native and Flatpak Steam sessions is inherently best-effort.
