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
