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
