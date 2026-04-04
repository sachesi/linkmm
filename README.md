# LinkMM

LinkMM is a Linux-first Bethesda mod manager built around **deterministic, state-driven rebuilds**.

## Core model

LinkMM does not deploy incrementally per action. Instead, it always recomputes game deployment from current state:

- active game profile
- profile-local mod enabled/order state
- profile-local plugin state
- profile-local generated output packages
- deployer rules (assets + plugins)

The final game folder is derived from this state only, not interaction history.

## Profiles / instances

Profiles are first-class state containers per game. Each profile owns:

- mod enabled/disabled and ordering
- plugin order + disabled list
- generated output package associations
- deployment ownership / backup state (profile-scoped)

Switching profile triggers a rebuild for that profile and isolates state from other profiles.

## Generated outputs

External tools (BodySlide, Pandora, Nemesis, etc.) are managed through generated output packages:

- explicit output-directory capture
- snapshot/diff capture for direct `Data/` writers
- ownership tracking per generated file
- deterministic inclusion in deployment and conflict resolution
- remove/cleanup/adopt workflows

## Tool runs

Tool launch goes through managed orchestration:

1. adapter + preflight validation
2. run execution
3. output capture/import (mode-dependent)
4. package update/replace
5. deployment rebuild

Tool adapters provide presets, validation and detection logic.

## Repository layout

- `src/core/*` — domain logic (deployment, mods, profiles, plugins, generated outputs, adapters, tool runs)
- `src/ui/*` — GTK/libadwaita UI
- `src/lib.rs` — core module entry for domain-oriented testing/use
- `src/main.rs` — UI app binary

## Running tests

Headless core-oriented tests (no UI feature):

```bash
cargo test --lib --no-default-features
```

Full app build/tests (requires GTK/libadwaita development packages):

```bash
cargo test
```

See `docs/developer-testing.md` for contribution/testing details.
