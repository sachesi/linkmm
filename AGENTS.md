# AGENT.md

Status: Canonical repository instructions for coding agents working on linkmm

> If a tool expects `AGENTS.md` instead of `AGENT.md`, copy this file verbatim.
> `AGENT.md` is the canonical version in this repository.

## Mission

You are working on **linkmm**, a Linux-first, GNOME-native mod manager focused on
Bethesda-style modding workflows.

The product goal is **not** to clone Windows-first tools exactly.
The goal is to provide a stronger Linux/GNOME-native experience while preserving
the workflows that matter most:

- stable install/deploy behavior
- Nexus/NXM support
- Proton/UMU compatibility
- profiles
- load order
- external tools

Read these project docs before making structural changes:

1. `docs/IMPLEMENTATION_TASKS.md`
2. `docs/storage-contract.md`
3. `docs/security-execution-policy.md`
4. `docs/compatibility.md`

---

## Working rules

## 1. One coherent change per PR

Keep scope tight.

Good:
- secret redaction layer + tests
- atomic installer contract implementation + tests
- GNOME Preferences window

Bad:
- “refactor installer, tweak UI, change packaging, and fix logs” in one PR

## 2. Preserve invariants

Before changing installer, deploy, profiles, or paths, verify your change preserves:

- no half-installed mods
- no secret leakage in logs/UI
- deterministic profile behavior
- deterministic deploy outcome
- typed, understandable failures

If your change threatens one of these, stop and refactor first.

## 3. Prefer explicit contracts over clever behavior

Prefer:
- small typed structs
- enums for operation state
- dedicated adapters for game/path resolution
- clear manifests

Avoid:
- hidden global state
- shell-string command construction
- “best effort” silent fallbacks that obscure bugs

## 4. Never log secrets

Treat these as sensitive:
- `nexus_api_key`
- NXM signed download parameters (`key`, `expires`, similar)
- future auth/session tokens

If you touch logs, command previews, network code, or settings UI,
add or update redaction tests.

## 5. Do not bypass the storage contract

Installer/deploy changes must follow staging -> validate -> commit -> register -> cleanup.

Do not:
- create final mod dirs before validation
- mark a mod installed before commit
- leave partial dirs after cancel/failure

---

## Priority order

When in doubt, prioritize work in this order:

### P0 — Reliability / safety
- CI gates
- secret redaction
- atomic installer/deploy behavior
- cancel/failure cleanup
- Nexus quota-safe behavior

### P1 — Domain correctness
- load order engine boundaries
- profile redeploy correctness
- external tools runner correctness
- FOMOD/archive regression handling

### P2 — GNOME-native UX / packaging
- Preferences/About
- AppStream/icon polish
- progress and status clarity
- Flatpak-friendly behavior

Do not spend time polishing secondary UX while P0 invariants are broken.

---

## File / module guidance

## Core areas likely to change

- `src/core/installer/*`
- `src/core/config.rs`
- `src/core/nexus.rs`
- `src/core/games.rs`
- deploy / profile / load-order modules
- UI adapters for settings, tools, load order

## Architectural expectations

### Installer
Must be structured around:
- staging temp dir
- validation pass
- atomic commit
- cleanup on failure/cancel

### Nexus integration
Must support:
- secret redaction
- bounded retry/backoff
- cache-aware behavior
- distinct handling for auth/rate-limit failures

### Tool runner
Must:
- use argv, not shell strings
- canonicalize executable/cwd
- use env allowlists
- render redacted launch previews

### UI
Must remain compatible with GTK4/libadwaita patterns and GNOME-style flows.

---

## Required checks before finishing

Run these when relevant:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

If you cannot run one of them in the current environment, say so explicitly in your summary.

For installer/deploy/security changes, also verify:
- tests were added/updated
- no new plaintext secrets in logs or UI strings
- docs updated if behavior changed

---

## Testing expectations

## Minimum rule

No behavior change without verification.

## What to add depending on change type

### Storage / installer / deploy
Add:
- unit tests for invariants
- integration or golden-style tests for realistic archives where possible
- cancel/failure cleanup coverage

### Logging / secrets / execution policy
Add:
- redaction tests
- allowlist / path validation tests
- shell-free command construction tests

### Profiles / load order
Add:
- deterministic sort tests
- missing-master / cycle diagnostics tests
- profile switch redeploy tests

### UI-only work
At minimum:
- state transformation tests where possible
- clear manual QA notes in PR summary

---

## How to write changes

## Prefer these patterns

### Typed results

```rust
struct SortResult {
    order: Vec<String>,
    diagnostics: Vec<Diagnostic>,
    applied_rules: Vec<AppliedRule>,
}
```

### Explicit execution plan

```rust
struct ExecutionPlan {
    executable: PathBuf,
    args: Vec<String>,
    cwd: PathBuf,
    env: BTreeMap<String, String>,
}
```

### Narrow helper APIs

```rust
fn ensure_path_within(path: &Path, allowed_roots: &[&Path]) -> Result<()>;
```

## Avoid these patterns

- concatenated shell commands
- giant functions mixing UI, IO, and business logic
- hidden filesystem side effects during “preview” operations
- silent fallback from invalid configuration to dangerous defaults

---

## Documentation duties

If your change alters behavior, update the relevant doc:

- storage model changes -> `storage-contract.md`
- execution/logging/path policy changes -> `security-execution-policy.md`
- supported scenarios or scope changes -> `compatibility.md`
- roadmap / acceptance criteria changes -> `IMPLEMENTATION_TASKS.md`

A code change without matching docs is incomplete if behavior changed materially.

---

## PR summary template

Use this structure in your final summary:

```text
Summary
- What changed

Why
- Which problem this solves
- Which contract/task it aligns with

Tests
- Commands run
- New tests added

Risks / follow-ups
- Any remaining gaps
```

---

## Current recommended work threads

These are the preferred bounded task slices:

1. `ci-baseline-and-gates`
2. `secret-redaction-and-config-hardening`
3. `atomic-installer-contract`
4. `fomod-regression-fixtures`
5. `nexus-cache-and-backoff`
6. `load-order-engine-refactor`
7. `profiles-redeploy-consistency`
8. `tools-runner-ux`
9. `gnome-preferences-about`
10. `appstream-and-packaging-polish`
11. `docs-drift-cleanup`

Stay within one thread unless explicitly instructed otherwise.

---

## Definition of done

A task is done only if:

- code quality checks pass, or inability is explained
- tests cover the changed behavior
- no secrets are exposed
- user-visible/documented behavior is updated in docs
- change scope remains coherent and reviewable

When in doubt, choose the safer, more explicit, more testable implementation.
