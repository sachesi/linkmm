# Compatibility

Version: 1.0  
Status: Working target / implementation guidance  
Audience: users, maintainers, Codex agents

## Purpose

This document defines **what linkmm is intended to support**, what is experimental,
and what is explicitly out of scope.

It should guide product decisions, not just describe current behavior.

When implementation differs from this document, open an issue or update the document.

---

## Scope summary

linkmm is a **Linux-first, GNOME-native mod manager** for Bethesda-style game modding,
with a strong focus on:

- native GTK4/libadwaita UX
- Nexus/NXM workflow support
- Proton/UMU-based game setups
- deterministic install/deploy/profile behavior
- load-order and external tool workflows

This project is not trying to be a drop-in clone of existing Windows-first managers.
It is trying to provide a **more native Linux/GNOME experience** while keeping the
core modding workflows practical.

---

## Support tiers

Use these tiers consistently in UI, docs, and issue triage.

## Tier A — Supported

Expected to work; bugs should be treated as regressions.

## Tier B — Experimental

May work; known gaps are acceptable while implementation matures.

## Tier C — Best effort

No compatibility promise; issues may be closed unless they align with roadmap goals.

## Tier D — Out of scope

Not a planned target.

---

## Platform compatibility

| Area | Tier | Notes |
|---|---|---|
| Linux desktop | A | Primary target |
| GNOME desktop | A | Primary UX target |
| Wayland session | A | Should be a first-class path |
| X11 session | B | Should work where GTK stack supports it |
| Steam Deck / Gamescope-style usage | B | Important target, but may need focused QA |
| Non-GNOME Linux desktops | B | App should run, but GNOME-native UX remains the design center |
| Windows host OS | D | Not a product target |
| macOS host OS | D | Not a product target |

---

## Packaging compatibility

| Package / delivery path | Tier | Notes |
|---|---|---|
| Local `cargo build` for development | A | Required for contributors |
| Native distro package (RPM/DEB/etc.) | B | Useful, but not the canonical UX path |
| Flatpak | A (target) | Preferred end-user delivery model for GNOME/Linux |
| AppImage / Snap | C | Not a priority unless contributors adopt them |

### Packaging expectations

Flatpak is the preferred long-term distribution target because it aligns with:

- GNOME software distribution expectations
- metadata/AppStream flow
- clearer permission modeling

Native packages remain valuable for development and distro integration.

---

## Game-family compatibility

## Initial target family

The target domain is Bethesda-style modding, especially titles with:

- `Data/` style content layout
- plugin/load-order workflows
- common modding tool chains
- Proton/Wine-based Linux play patterns

## Target tiers by family

| Game family | Tier | Notes |
|---|---|---|
| Skyrim Special Edition / Anniversary-style setups | A | Primary target class |
| Fallout 4 | A | Primary target class |
| Skyrim Legendary / classic variants | B | Important but may require specific path/format handling |
| Fallout: New Vegas | B | Valuable target, requires compatibility-specific validation |
| Other Bethesda-family mod workflows | B/C | Case-by-case depending on paths, plugin formats, and tooling |
| Non-Bethesda games | C/D | Only if architecture generalization comes naturally |

### Rule

Do not generalize the product around “any game can be modded” until the
Bethesda-family workflows are stable.

---

## Store / installation-source compatibility

| Installation source | Tier | Notes |
|---|---|---|
| Steam on Linux with Proton | A | Core use case |
| Non-Steam setup using UMU-compatible workflow | A/B | Important target |
| Steam compatdata-based prefixes | A | Core path model |
| GOG / custom Wine prefixes | B | Valuable, but adapter layer must mature |
| Windows Store / Game Pass editions | D | Not a target for tool/script-extender heavy workflows |

### Why Game Pass / Windows Store is out of scope

Many common script-extender and modding workflows assume layouts and executables
that do not match Windows Store restrictions. Those paths should not be advertised
as supported until proven otherwise.

---

## Mod source compatibility

| Source | Tier | Notes |
|---|---|---|
| Nexus Mods API metadata | A | Important integration target |
| `nxm://` one-click handler flow | A | Important integration target |
| Local archive import | A | Must always be available |
| Drag-and-drop local files | B | Good UX improvement path |
| Other online repositories | C | Only with a clear maintenance plan |

### Nexus constraints

Nexus integration must remain respectful of:
- rate limits
- acceptable use policies
- short-lived signed download parameters

The app should be designed to work even when Nexus integration is unavailable,
via local archive import.

---

## Archive format compatibility

| Archive type | Tier | Notes |
|---|---|---|
| ZIP | A | Baseline required |
| 7z | A | Baseline required, including large archives |
| RAR / unrar-backed flows | B | Supported where implementation and licensing constraints allow |
| Multi-part archives | C | Best effort only unless explicitly prioritized |
| Exotic/self-extracting formats | D | Out of scope |

### Archive expectations

Archive handling must correctly support:
- wrapper directories
- `Data/` root detection
- FOMOD-like structure discovery
- rejection of unsafe traversal paths

---

## Installer / mod format compatibility

| Format / behavior | Tier | Notes |
|---|---|---|
| Plain archive with `Data/` content | A | Core |
| Wrapper dir around `Data/` | A | Core |
| FOMOD installer workflows | A/B | Important target with ongoing regression coverage |
| Archive with no valid installable content | A | Must fail clearly, not half-install |
| Script-heavy custom installer executables | D | Out of scope for native pipeline |

### FOMOD stance

FOMOD support is important, but the quality bar is high:
- no half-installed mods
- no empty mod dirs on failure
- deterministic selection result
- regression fixtures for known edge cases

---

## Deployment compatibility

| Deployment mode | Tier | Notes |
|---|---|---|
| Symlink deploy | A | Preferred baseline |
| Hardlink deploy | B | Useful where filesystem constraints permit |
| Copy deploy | B | Fallback / compatibility mode |
| Overlay/FUSE-based deployment | C | Interesting, but not near-term core |

### Deploy expectations

Regardless of mode, deploy must be:
- deterministic
- explainable
- profile-scoped
- rebuildable from source state

---

## Profile compatibility

| Feature | Tier | Notes |
|---|---|---|
| Multiple profiles per game | A | Core target |
| Enabled-mod differences per profile | A | Core target |
| Order differences per profile | A | Core target |
| Tool configuration overrides per profile | B | Valuable |
| Cross-game shared profiles | D | Out of scope |

---

## Load-order compatibility

| Capability | Tier | Notes |
|---|---|---|
| Read/write game-specific plugin state | A | Core |
| Deterministic sort engine | A | Core |
| Diagnostics for conflicts / missing masters / cycles | A | Core target |
| Local metadata rules (`load after`, groups, etc.) | B | Important follow-up |
| Full parity with every mature Windows ecosystem feature | C | Not required initially |

### Philosophy

The app should provide:
- a trustworthy engine
- understandable explanations
- enough metadata editing for real workflows

It does not need immediate feature parity with every long-established tool.

---

## External tools compatibility

| Tool class | Tier | Notes |
|---|---|---|
| LOOT-like sorting workflow | A/B | Important target |
| xEdit-style workflow | A/B | Important target |
| Script extenders (launcher awareness / compatibility guidance) | B | Important, but varies by game/version |
| Arbitrary custom external tools | B | Supported through explicit configuration |
| Shell-command templates by default | D | Unsafe by default; not a standard path |

### Tool runner expectations

A supported tool run must:
- resolve the correct prefix / Proton context
- display a redacted launch preview
- fail clearly on invalid paths
- not rely on shell interpolation by default

---

## Desktop UX compatibility

| UX area | Tier | Notes |
|---|---|---|
| GTK4/libadwaita main UI | A | Core |
| GNOME HIG-aligned preferences/about | A | Core target |
| Keyboard navigation | A | Important baseline |
| Screen-reader/accessibility polish | B | Must improve over time |
| Touch-first tablet UX | C | Nice-to-have |

---

## Filesystem and environment compatibility

| Environment | Tier | Notes |
|---|---|---|
| Same-filesystem staging and final storage | A | Best atomic behavior |
| Cross-filesystem mod/data roots | B | Must be supported with fallback commit logic |
| Read-only game roots | C | Limited by deploy mode |
| Network filesystems | C | Best effort only |
| Case-insensitive edge cases | C | Needs caution, not a design center |

---

## Known non-goals

These are currently out of scope unless a roadmap decision changes them:

- native Windows support
- full parity with every Windows-only mod manager feature
- embedded shell scripting as a normal execution path
- arbitrary game-agnostic plugin ecosystems
- cloud sync as a core feature
- automatic support claims for every archive found on the internet

---

## Compatibility requirements for new features

Any new feature PR must answer:

1. Which support tier does this improve?
2. Does it raise the bar for a Tier A workflow?
3. Does it accidentally expand scope into Tier C/D territory?
4. What tests prove the intended compatibility claim?

If those answers are unclear, the PR is not ready.

---

## Definition of done for compatibility-affecting changes

A change affecting compatibility is not done until:

- the impacted tier is stated in the PR
- tests or QA notes reflect the claim
- user-facing docs are updated
- unsupported scenarios are not silently advertised as supported
