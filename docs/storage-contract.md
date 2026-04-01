# Storage Contract

Version: 1.0  
Status: Proposed  
Audience: core, installer, deployment, profiles, UI, Codex agents

## Purpose

This document defines the **storage model and invariants** for linkmm.

The goal is to make installation, deployment, profile switching, cleanup, and recovery
**deterministic, atomic where possible, and debuggable**.

This contract is intentionally strict: if implementation and this document disagree,
implementation should be updated unless there is an explicit architecture decision.

---

## Design principles

1. **One source of truth per concern**
   - Archive/download state lives in `downloads/`
   - Extracted mod content lives in `mods/`
   - Active game-visible projection lives in `deploy/`
   - User intent lives in `config.toml` + per-profile state

2. **No half-installed mods**
   - A mod is either:
     - not present
     - staged but not committed
     - fully committed
   - There must be no visible “installed” record for a mod whose content was not committed.

3. **Deploy is reproducible**
   - `deploy/` is a projection of:
     - enabled mods
     - profile order
     - conflict rules
     - game target paths
   - If `deploy/` is removed, it must be reconstructible from config + mod content.

4. **Profile switch is transactional at the app level**
   - A profile switch must not leave mixed content from two profiles.

5. **Recovery must be automatic**
   - Temp directories are disposable.
   - Journals/state markers are minimal and machine-readable.

---

## Root layout

Unless overridden by explicit user settings, use XDG-style locations.

## Global app roots

```text
$config_root/
  config.toml
  games/
    <game_id>/
      profiles/
        <profile_id>.toml
      tools/
        <tool_id>.toml

$data_root/
  downloads/
    <game_id>/
      <download_id>/
        archive.<ext>
        metadata.json
  mods/
    <game_id>/
      <mod_id>/
        manifest.json
        files/
          ...
  deploy/
    <game_id>/
      <profile_id>/
        manifest.json
        links/
          ...
  state/
    jobs/
      <job_id>.json
  logs/
    app.log

$cache_root/
  nexus/
  thumbnails/
  extracted-metadata/

$runtime_root/
  tmp/
    <job_id>/
```

## Meanings

- `config_root`:
  durable user intent and settings.
- `data_root`:
  durable, user-owned application data.
- `cache_root`:
  safe to delete; reconstructible.
- `runtime_root/tmp`:
  must never be treated as durable state.

---

## IDs and naming rules

## `game_id`

Stable internal slug, for example:

- `skyrimse`
- `skyrim`
- `fallout4`
- `falloutnv`

Rules:
- lowercase ASCII
- no spaces
- immutable after release

## `profile_id`

Opaque stable identifier, not the display name.

Rules:
- UUID, ULID, or equivalent stable random ID
- display name stored separately

## `mod_id`

Opaque internal ID.

Rules:
- not derived solely from Nexus mod ID
- may include source hint in metadata
- must remain stable across rename of display name

## `job_id`

Ephemeral operation ID for download/install/extract/deploy jobs.

---

## Persistent objects

## Download record

Stored at:

```text
data/downloads/<game_id>/<download_id>/metadata.json
```

Fields:

```json
{
  "download_id": "stable-id",
  "game_id": "skyrimse",
  "source": "nexus",
  "source_mod_id": 1234,
  "source_file_id": 5678,
  "archive_name": "foo.7z",
  "archive_sha256": "hex",
  "downloaded_at": "RFC3339",
  "size_bytes": 123
}
```

Rules:
- archive path and metadata must agree
- if archive is missing, record is invalid and should be flagged as broken, not silently used

## Installed mod record

Stored at:

```text
data/mods/<game_id>/<mod_id>/manifest.json
```

Fields:

```json
{
  "mod_id": "stable-id",
  "game_id": "skyrimse",
  "display_name": "Example Mod",
  "version": "1.2.0",
  "source": {
    "kind": "nexus",
    "mod_id": 1234,
    "file_id": 5678
  },
  "installed_at": "RFC3339",
  "content_root": "files",
  "file_count": 42,
  "hash_strategy": "sha256-manifest-v1"
}
```

Rules:
- `manifest.json` is written only after content commit succeeds
- `file_count` must reflect committed files, not staged files

## Deployment manifest

Stored at:

```text
data/deploy/<game_id>/<profile_id>/manifest.json
```

Fields:

```json
{
  "game_id": "skyrimse",
  "profile_id": "stable-id",
  "generated_at": "RFC3339",
  "mods_in_order": ["mod-a", "mod-b"],
  "entries": [
    {
      "relative_target": "Data/textures/foo.dds",
      "winner_mod_id": "mod-b",
      "source_relpath": "files/Data/textures/foo.dds",
      "mode": "symlink"
    }
  ]
}
```

Rules:
- this file is the authoritative description of what deploy generated
- if filesystem contents disagree with the manifest, deploy is considered dirty and must be rebuilt

---

## Install contract

## Stages

Every install operation must follow this order:

1. **Acquire**
   - resolve archive path
   - verify archive exists
   - compute or verify hash if available

2. **Stage**
   - create unique temp directory under `runtime/tmp/<job_id>/`
   - extract or materialize selected files there

3. **Validate**
   - ensure there is meaningful content
   - detect `Data/` root correctly
   - strip wrappers like top-level archive directories when appropriate
   - reject empty selection / empty result
   - build manifest candidate

4. **Commit**
   - move staged content into final mod directory with atomic rename when same filesystem
   - only after content move succeeds, write final `manifest.json`

5. **Register**
   - update config/profile references
   - emit UI success state

6. **Cleanup**
   - remove temp directory
   - remove failed partial output

## Forbidden behavior

The installer must **not**:

- create the final mod directory before validation passes
- mark a mod installed before commit succeeds
- leave empty mod directories after cancel/failure
- mix staging and final storage roots

---

## Deploy contract

## Inputs

Deploy consumes:

- game definition
- active profile
- enabled mod set
- load/conflict order
- deployment mode
- target game path

## Output modes

Initial supported deployment modes should be explicit:

- `symlink`
- `hardlink` (only where safe and same filesystem)
- `copy` (fallback / compatibility mode)

Mode must be recorded in deploy manifest.

## Rules

1. Generate into a fresh staging directory first:
   ```text
   runtime/tmp/<job_id>/deploy/
   ```

2. Build complete projected tree.

3. Validate:
   - all source files exist
   - collisions resolve deterministically
   - no cycles or broken source references

4. Commit by replacing the profile deploy root:
   ```text
   data/deploy/<game_id>/<profile_id>/
   ```

5. Only then mirror/apply to the actual game-visible directory if required by chosen deploy strategy.

## Conflict rule

Later winning mod order must be deterministic and profile-scoped.

At minimum:
- the deploy engine must be able to answer:
  - who won
  - who lost
  - why

---

## Profiles contract

A profile must contain or reference:

- enabled mod set
- mod priority / order
- load order metadata
- tool overrides
- deploy settings overrides (if any)

A profile switch must:

1. persist pending changes from current profile
2. load target profile state
3. rebuild deploy projection
4. refresh UI from resulting state

A profile switch must not:

- reuse old deploy manifest
- silently merge enabled mods across profiles
- mutate global state except where explicitly declared global

---

## Game path contract

Game-specific mutable files must be resolved through a game adapter layer.

The rest of the application must not hardcode Steam / Proton / prefix paths directly.

The adapter must provide:

```rust
struct ResolvedGamePaths {
    game_root: PathBuf,
    data_dir: PathBuf,
    plugins_txt: PathBuf,
    appdata_local_dir: PathBuf,
    prefix_root: Option<PathBuf>,
}
```

Rules:
- path resolution must be deterministic for a given game configuration
- all resolved paths must be canonicalized before use where possible
- failures must be surfaced as typed errors

---

## Cache contract

Caches are optional and disposable.

Allowed caches:
- Nexus API responses
- thumbnails
- extracted archive metadata
- derived load-order diagnostics

Caches must:
- include versioning in key format
- include fetched/generated timestamp
- never be the only source of truth for user state
- be safe to delete without data loss

Caches must not:
- store secrets in plaintext if avoidable
- store NXM `key` / `expires` in reusable durable caches

---

## Temp and recovery contract

## Temp directories

All temp paths must be created under:

```text
runtime/tmp/<job_id>/
```

Allowed contents:
- extracted archive content
- deploy staging tree
- transient generated manifests
- child-process working files

## Startup recovery

On app startup:

1. scan `runtime/tmp/`
2. remove stale temp directories
3. scan `state/jobs/` for interrupted jobs
4. mark them as failed/recoverable in UI if needed
5. scan `mods/` for manifest/content mismatch
6. scan `deploy/` for dirty manifests

Never attempt silent destructive repair of user content beyond temp cleanup.

---

## File integrity and manifests

Each committed mod should support a content manifest.

Recommended format:

```json
{
  "version": 1,
  "files": [
    {
      "relative_path": "Data/foo/bar.txt",
      "sha256": "hex",
      "size": 1234
    }
  ]
}
```

Uses:
- repair
- broken-link detection
- export/import
- debugging conflict resolution

This may be incremental, but new code should assume manifest support exists or is being added.

---

## Removal contract

Removing a mod must:

1. verify mod is not locked by active job
2. remove from all profiles or mark references broken
3. rebuild affected deploys
4. remove mod directory
5. remove derived cache entries

The app must not:
- delete shared downloads unless user explicitly asked
- leave broken deploy entries after mod removal

---

## Backup / export contract

Future export/import features should treat these as exportable units:

- profile config
- enabled mod list
- order metadata
- tool config
- references to installed mods

Exports must not include:
- secrets unless explicitly requested
- ephemeral temp/runtime state

---

## Invariants checklist

The following must always hold:

- No installed mod without `manifest.json`
- No `manifest.json` pointing to missing `files/`
- No deploy manifest without matching generated tree
- No profile referencing unknown game ID
- No active profile referencing non-existent profile ID
- No secrets written into manifests
- No temp dir used as durable state

---

## Error handling

Use typed error classes:

- `StorageError::MissingRoot`
- `StorageError::InvalidManifest`
- `StorageError::CrossDeviceAtomicMoveUnavailable`
- `StorageError::DirtyDeploy`
- `StorageError::BrokenProfileReference`

UI must translate these into user-facing messages, but storage layer returns structured errors.

---

## Migration policy

Whenever a durable format changes:

1. bump schema version
2. add migration logic
3. keep backward read compatibility where reasonable
4. never silently drop user-owned data

---

## Implementation priorities

Highest priority items aligned with this contract:

1. atomic installer pipeline
2. deploy manifest and dirty-detect behavior
3. profile switch redeploy contract
4. startup recovery and temp cleanup
5. file manifests for installed mods

---

## Definition of done for storage changes

A storage-related PR is not done until:

- unit tests cover the new invariant
- interruption/cancel path is tested where relevant
- no partial directories remain after failure
- docs in this file are updated if behavior changed
