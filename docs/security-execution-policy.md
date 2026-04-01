# Security Execution Policy

Version: 1.0  
Status: Proposed  
Audience: core, tools runner, Nexus/NXM integration, Codex agents

## Purpose

This document defines how linkmm may execute child processes, handle secrets,
resolve paths, and interact with external tools.

It exists to reduce the risk of:

- secret leakage
- path traversal / confused-deputy bugs
- accidental execution of the wrong binary
- unsafe environment inheritance
- destructive operations against arbitrary directories
- quota abuse against third-party APIs

---

## Threat model

We assume:

- archives and mod metadata are untrusted input
- tool paths provided by users may be malformed or point somewhere unexpected
- Nexus/NXM values include sensitive tokens
- Proton/Wine/UMU environments may contain many unrelated host variables
- external tools may produce large, noisy, or sensitive logs

We do **not** assume:

- every mod archive is well-formed
- every external tool is benign
- the host environment is clean or minimal

---

## Core security principles

1. **Least privilege**
   - Pass only the environment variables and filesystem access needed for the operation.

2. **Canonicalize before trust**
   - Resolve executable, working directory, and user-selected paths before use.

3. **Never log secrets**
   - If a value can authenticate, authorize, or reproduce a privileged request, redact it.

4. **Explicit allowlists beat inheritance**
   - Prefer a small allowlist of environment variables over passing the host environment through unchanged.

5. **Typed failures**
   - Security-relevant blocks must fail closed and return explicit errors.

---

## Sensitive values

Treat all of the following as secrets:

- `nexus_api_key`
- NXM download parameters such as `key`, `expires`, and equivalent signed download parameters
- OAuth/session tokens if introduced later
- any auth cookies or bearer tokens
- future telemetry API keys
- tool arguments that embed credentials

These must never be shown raw in:
- UI
- logs
- crash reports
- debug dumps
- copied command previews

---

## Logging redaction policy

All user-visible and file logs must pass through a central redaction layer.

Minimum redaction targets:
- `apikey=...`
- `key=...`
- `expires=...`
- `Authorization: Bearer ...`
- any configured Nexus API key value
- any env var value from secret sources

Recommended interface:

```rust
fn redact_secrets(input: &str, known_secrets: &[&str]) -> Cow<'_, str>;
```

Rules:
- Redaction runs before persistence and before UI emission
- Redaction must be deterministic
- Redaction must not panic on malformed UTF-8 boundaries if fed lossy text
- Tests must cover URL query strings, headers, and plain text blocks

---

## Environment allowlist

Child processes must not inherit the full host environment by default.

Allowed environment variables should be explicitly constructed per launch.

## Baseline allowlist

Only include variables required for correct execution, for example:

- `HOME`
- `XDG_CONFIG_HOME`
- `XDG_DATA_HOME`
- `XDG_CACHE_HOME`
- `LANG`
- `LC_ALL`
- `PATH` (sanitized)
- `TMPDIR`

## Game/tool specific allowlist

Only when relevant:
- `WINEPREFIX`
- `PROTONPATH`
- `STEAM_COMPAT_DATA_PATH`
- `STEAM_COMPAT_CLIENT_INSTALL_PATH`
- `LINKMM_PROFILE_ID`
- `LINKMM_GAME_ID`

## Forbidden by default

Do not pass through arbitrary variables such as:
- `AWS_*`
- `GITHUB_*`
- `SSH_*`
- `DBUS_SESSION_BUS_ADDRESS` unless clearly required
- shell history variables
- unrelated service credentials
- proxy credentials unless explicitly configured for that network request path

---

## Executable path policy

External tool execution must follow this order:

1. resolve configured executable path
2. canonicalize path
3. verify file exists
4. verify file type is executable regular file where applicable
5. verify path is not inside disallowed transient locations unless explicitly permitted
6. only then execute

Recommended error names:
- `ExecutionPolicyError::PathDoesNotExist`
- `ExecutionPolicyError::NotExecutable`
- `ExecutionPolicyError::PathNotAllowed`

## PATH search

Do not rely on ambient shell lookup for critical tools when a fully resolved path is available.

For template tools:
- first prefer explicit user-configured path
- second prefer known resolved path from compatibility adapter
- only lastly perform controlled PATH lookup

---

## Working directory policy

Working directories must be canonicalized before use.

Allowed working directory categories:
- resolved game root
- resolved tool directory
- app-controlled temp directory for the current job

Forbidden by default:
- `/`
- user home root as a fallback
- unrelated mount roots
- deleted or non-canonical symlink targets

If canonicalization fails, block execution.

---

## Temporary and portal-like paths

Temporary execution paths may be used only for:

- archive extraction
- deploy staging
- tool scratch output
- generated patch files

They must live under the app runtime temp root for the active job.

If using user-selected document portal paths or other transient mounts:
- canonicalize them
- copy required data into app-controlled temp storage if long-running access is needed
- never store portal/transient paths as permanent durable references without explicit confirmation

---

## Archive handling policy

Archives are untrusted input.

The extractor must defend against:
- path traversal (`../`)
- absolute paths
- duplicate normalized paths
- case-collision ambiguities where relevant
- symlink entries that escape extraction root
- device file / special file extraction

Rules:
- extract only into app-controlled temp directory
- normalize candidate paths before writing
- reject entries escaping extraction root
- reject or ignore unsupported special entries
- never extract directly into final mod directory

---

## Network policy

## Nexus API

All Nexus API usage must be rate-limit aware.

Rules:
- use caching for repeated metadata endpoints
- use retry with backoff for transient failures only
- never retry signed download actions blindly without bounds
- surface 401/403/429 distinctly

Signed NXM parameters must be treated as sensitive and short-lived.
Do not persist them beyond what is needed to complete the current action.

## Telemetry

If usage counters are added, they must be:
- opt-in or clearly disclosed
- non-sensitive
- aggregated where practical
- free of file paths, secrets, and personal identifiers unless explicitly justified

---

## Child process policy

Every child-process launch must be represented by a structured execution plan.

Suggested type:

```rust
struct ExecutionPlan {
    executable: PathBuf,
    args: Vec<String>,
    cwd: PathBuf,
    env: BTreeMap<String, String>,
    timeout: Option<Duration>,
    log_policy: LogPolicy,
}
```

Before launch:
- validate executable
- validate cwd
- construct allowlisted env
- render a redacted preview for UI/logs
- attach cancellation hooks where supported

After launch:
- capture stdout/stderr safely
- apply redaction before persistence/display
- enforce timeout or cancellation semantics if configured

---

## Tool runner constraints

User-configured tools such as LOOT/xEdit-like workflows must run with:

- explicit resolved path
- explicit resolved prefix/game context
- no shell interpolation
- arguments passed as structured argv, not concatenated shell strings

Forbidden:
- `sh -c "...user string..."`
- `bash -lc "...user string..."`
- storing single-string command lines and evaluating them through a shell by default

If advanced shell mode is ever added, it must be:
- clearly labeled unsafe/advanced
- opt-in
- visually distinct
- excluded from templates

---

## Delete / write safety rules

Any operation that deletes or overwrites files must be scoped to known roots.

Allowed destructive targets:
- app temp directories
- app deploy directories
- app-managed installed mod directories
- app-managed cache directories

Before delete/replace:
1. canonicalize path
2. verify path is within one of the allowed roots
3. refuse otherwise

Recommended helper:

```rust
fn ensure_path_within(path: &Path, allowed_roots: &[&Path]) -> Result<(), ExecutionPolicyError>;
```

Never recursively delete:
- arbitrary user-selected roots
- game root itself
- prefix root itself
- home directory
- mount roots

---

## Secret storage policy

Short term:
- support config-file storage only as legacy compatibility if already present
- do not echo stored values back in plaintext

Preferred:
- environment override support for CI/packaging/development
- future secret-service integration if adopted

At minimum:
- UI must mask secrets
- logs must redact secrets
- export/diagnostic bundles must omit secrets

---

## Crash reporting and diagnostics

Diagnostics bundles must exclude:
- API keys
- signed download URLs
- env dumps
- unrelated absolute paths where not needed

If including file paths for debugging:
- prefer relative paths within app-managed roots
- strip user home prefix where practical for display

---

## Cancellation policy

Long-running operations must be cancellable.

On cancel:
- terminate child processes gracefully first
- escalate only if needed
- clean temp output
- do not commit partial final state

A cancelled operation must end in:
- `Cancelled` state
- no registered final mod/deploy change

---

## Compatibility-mode exceptions

Any exception to this policy must be:
- documented
- scoped to a specific compatibility case
- justified in code comments and commit message

Examples:
- a tool requiring an otherwise disallowed env var
- a portal path needing temporary persistence during a job

There must be no silent “temporary” exceptions.

---

## Required tests

Security-sensitive changes are not done without tests.

Minimum test coverage:
- secret redaction for URL/query/header/plain text cases
- canonicalization failure behavior
- allowed-root delete guard
- execution-plan env construction
- shell-free argv construction
- archive traversal rejection
- cancel leaves no committed partial state

---

## Definition of done

A PR touching execution, tools, downloads, or logging is not done until:

- secrets are redacted in tests
- path validation is covered
- shell injection is not possible through normal code paths
- logs are reviewed for accidental plaintext output
- this document is updated if behavior changed
