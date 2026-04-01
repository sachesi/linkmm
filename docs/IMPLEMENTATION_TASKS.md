# linkmm Implementation Tasks Plan

_Last updated: 2026-04-01_

## Goal
Create a practical execution plan for **what to implement, reimplement, fix, and remove** in linkmm, based on the provided research guide.

---

## 1) Implement (new capabilities)

### 1.1 Engineering baseline (P0)
- Add a standard CI workflow (`build + fmt + clippy + test`) for pull requests.
- Add release checklist and versioning workflow.
- Add issue templates (bug report, feature request, regression).

**Acceptance criteria**
- PRs are blocked on failing CI checks.
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` run automatically.

### 1.2 Nexus API resiliency (P0)
- Implement cache layer for frequent Nexus endpoints (TTL-based).
- Add retry/backoff behavior for transient API failures.
- Add usage telemetry counters for “API requests saved by cache” (non-sensitive).

**Acceptance criteria**
- Cache hit avoids HTTP call.
- Respect rate limits in load test scenario.

### 1.3 UX / GNOME polish (P2)
- Implement GNOME-native Preferences and About windows.
- Add AppStream metadata (`.metainfo.xml`) and proper icon set.
- Improve progress state messaging for downloads/install/deploy.

**Acceptance criteria**
- Preferences and About are accessible from app menu.
- App metadata appears correctly in software centers.

### 1.4 Tools and profile experience (P1)
- Implement external tools runner templates (LOOT/xEdit-style workflows).
- Implement stronger profile switching behavior (enabled mods + load order + deploy state).

**Acceptance criteria**
- Profile switch is deterministic and redeploys correctly.
- Tool “test run” reports resolved env and launch result.

---

## 2) Reimplement (architectural refactors)

### 2.1 Atomic install/deploy pipeline (P0)
Reimplement installer/deployment flow around a strict atomic contract:
1. Stage to temp directory
2. Validate extracted content
3. Commit with atomic move/rename
4. Register mod only on success
5. Cleanup temp/partial data on failure or cancel

**Acceptance criteria**
- No half-installed mods after interruption/failure.
- Failure/cancel leaves no empty mod directories.

### 2.2 Load order engine boundaries (P1)
Reimplement load-order logic as two layers:
- Core engine: deterministic sort + diagnostics + applied rules
- UI adapter: human-readable explanation and conflict display

**Acceptance criteria**
- Same input set always yields same sorted result.
- Cycles/missing masters produce explicit diagnostics.

### 2.3 Secret handling path (P0)
Reimplement handling of sensitive values (`nexus_api_key`, NXM `key/expires`) with centralized redaction and safe display.

**Acceptance criteria**
- Sensitive tokens are redacted in logs/UI.
- API key can be provided via env override for CI/packaging contexts.

---

## 3) Fix (defects, reliability gaps, documentation drift)

### 3.1 FOMOD/archive reliability (P0)
- Fix remaining edge-cases for Data root detection and wrapper directories.
- Fix regressions around large 7z extraction performance.
- Fix cancel behavior to be responsive during long extraction.

### 3.2 Documentation accuracy (P1)
- Fix stale file-path references in internal notes.
- Align docs with current module structure (`src/core/installer/*`).

### 3.3 Operational safety (P0)
- Fix logging policy to prevent accidental secret disclosure.
- Fix API call patterns that can aggressively consume quota.

**Acceptance criteria**
- Regression tests cover known historical bugs.
- Documentation references only existing files.

---

## 4) Remove (debt, obsolete behavior, risky patterns)

### 4.1 Remove obsolete internal references (P1)
- Remove references to files that no longer exist.
- Remove duplicated/contradictory guidance in notes.

### 4.2 Remove non-actionable CI workflows as quality gates (P1)
- Keep agent/automation workflows if useful, but remove them as implied substitutes for compile/test/lint CI gates.

### 4.3 Remove plaintext exposure patterns (P0)
- Remove any UI/log output path that can show raw API secrets.

---

## 5) Execution roadmap (10 weeks)

## Phase A (Weeks 1-2): Foundation
- CI gates, AGENTS/CODEX process doc, issue templates.
- Secret redaction baseline.

## Phase B (Weeks 3-5): Core reliability
- Atomic install/deploy reimplementation.
- FOMOD/7z regression suite and cancel semantics.

## Phase C (Weeks 6-7): Domain correctness
- Load order engine/UI boundary refactor.
- Profile switch correctness and redeploy behavior.

## Phase D (Weeks 8-9): UX and packaging
- Preferences/About integration.
- AppStream and icon packaging updates.

## Phase E (Week 10): Release hardening
- End-to-end QA on Nexus, NXM, Proton/UMU scenarios.
- Release checklist run and stabilization fixes.

---

## 6) Work threads for Codex (one thread = one coherent PR)

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

Each thread must include:
- Scope statement
- Acceptance criteria
- Automated checks
- Rollback notes

---

## 7) Definition of Done (for every task)

- Code/build checks pass in CI.
- New/changed behavior has tests (unit/integration/golden where applicable).
- No secrets in logs.
- User-visible changes are documented.
- Task linked to clear acceptance criteria from this plan.

