---
name: Steam redirector plan for Bethesda games
description: Research-backed plan for making Steam launch linkmm first, then the real game, while preserving Proton/Steam integration and FUSE ownership
type: project
originSessionId: ffcce041-18da-4075-839c-2ff9ffcaf565
---

## Goal

Support this launch shape for Steam-owned Bethesda games:

1. Steam launches `linkmm`
2. `linkmm` mounts the FUSE VFS
3. `linkmm` launches the real target inside Steam's Proton environment
4. `linkmm` tracks the descendant process tree
5. `linkmm` unmounts only after the final relevant process exits

This is the only model that fits `linkmm`'s FUSE ownership requirement. A later "launch the EXE yourself after Steam already started once" model is weaker and not the one to build around.

## Scope decision

Drop all games outside the exact six-game target set from this effort.

Working scope:

- Skyrim Anniversary Edition / Special Edition
- Fallout 4
- Fallout 3
- Fallout: New Vegas
- Oblivion
- Morrowind

Out of scope:

- every other Bethesda title not listed above
- all non-Bethesda games
- generic Steam game handling
- launcher-agnostic abstractions meant to support arbitrary third-party games

Reason:

- `linkmm` is already a Bethesda-focused mod manager
- the FUSE ownership and launch-chain assumptions are Bethesda-shaped
- broad game support would force weaker abstractions and more edge cases before the Steam redirector path is stable
- the listed six games are enough to validate and ship a coherent product direction
- supporting the rest of Bethesda's catalog would create extra launcher, engine, and store-path cases that are not needed for this plan

## Research summary

### What already exists in the wild

The strongest prior art is the Linux Mod Organizer 2 installer and its Steam redirector model.

- The installer's documented normal flow is: install the game on Steam, install the MO2 integration, then "Run the game on Steam and Mod Organizer 2 should start."
- Its docs describe a custom Steam integration/redirector that makes Steam launch MO2 first, while MO2 then launches the actual game executable.
- Its post-install docs explicitly support Steam launch options that tell MO2 to skip the UI and directly launch a chosen executable such as `SKSE` or `Fallout Launcher`.
- Its docs strongly advise against launching MO2 outside Steam because Proton's Steam-provided environment matters.

This is close to the shape `linkmm` needs. The important point is not "MO2 works", but "Steam -> wrapper/manager -> real game" is already a proven pattern for Bethesda titles on Linux/Proton.

### Why this matters for linkmm

`linkmm` must own the VFS lifecycle. That means:

- the mount must be live before the game process tree touches `Data/`
- the mount must remain live while launchers, script extenders, helper EXEs, and the final game EXE run
- cleanup must happen after the last relevant descendant exits, not after the first stub exits

That rules out designs where Steam owns the meaningful process tree and `linkmm` only observes from the side.

## Game-by-game feasibility

### Skyrim Anniversary Edition

Status: **best first target**

Reasoning:

- On Steam, Anniversary Edition is still the Skyrim Special Edition app/runtime with Anniversary content layered on top.
- The MO2 Linux installer marks Skyrim Special Edition as working for gameplay, script extenders, and ENB.
- The docs explicitly show direct Steam launch options for `SKSE`, which is a strong signal that Steam -> wrapper -> extender -> game is viable here.

Conclusion:

- `linkmm` should target Skyrim AE via the Skyrim SE Steam app (`489830`) and support launching either the stock launcher/game path or `SKSE` as the chosen target.

### Fallout 4

Status: **strong candidate**

Reasoning:

- The MO2 Linux installer marks Fallout 4 as working.
- There is public Linux guidance for running Fallout 4 on Steam/Proton with MO2 through Steam.
- Fallout 4 is new enough to matter, but old enough that there is already a mature Linux modding path around Proton + Steam redirection.

Conclusion:

- `linkmm` should treat Fallout 4 as a tier-1 supported Steam redirector game, just after Skyrim AE.

### Fallout 3

Status: **good candidate**

Reasoning:

- The MO2 Linux installer marks Fallout 3 as working.
- Community guidance shows Steam launch options that route Steam through MO2 while preserving Steam overlay, playtime, and automatic close behavior.

Conclusion:

- `linkmm` can likely support Fallout 3 with the same redirector pattern, but expect extra launcher/version handling.

### Fallout: New Vegas

Status: **candidate with caveat**

Reasoning:

- The MO2 Linux installer marks New Vegas gameplay as "Fullscreen Only", but script extender and ENB support are marked working.
- Their post-install docs explicitly use "Fallout Launcher" as the example direct target for New Vegas.

Conclusion:

- The redirector model appears viable, but `linkmm` should expect edge cases around fullscreen/window transitions and launcher behavior.

### Oblivion

Status: **probably viable**

Reasoning:

- The MO2 Linux installer marks Oblivion gameplay as working, with a caveat that some plugins need manual setup.

Conclusion:

- Worth supporting after Skyrim AE and Fallout 4. The risk is not Steam ownership so much as old-tool/plugin quirks.

### Morrowind

Status: **uncertain**

Reasoning:

- The MO2 Linux installer lists Morrowind as not tested.
- Broader Linux community practice often points users toward OpenMW instead of the original Morrowind.exe path.
- Evidence exists that MO2 can be used with Morrowind in some setups, but the specific Steam -> Linux wrapper -> Proton original-engine flow is less well established than for the other Bethesda titles.

Conclusion:

- Do not make Morrowind part of the first implementation target set.
- Treat it as follow-up research or possibly a separate path, especially if `linkmm` later wants to support OpenMW directly.

## Main constraints discovered

### 1. Steam's Proton environment must remain the parent environment

The most consistent guidance from Linux MO2 docs is that running the manager outside Steam is the weaker path. The wrapper should be entered from Steam, not the other way around.

Implication for `linkmm`:

- do not build around `protontricks-launch` or `umu-run` as the normal Steam path for these games
- use them only for debugging or fallback workflows

### 2. `linkmm` must stay alive as supervisor

Steam launching a tiny redirector that `exec`s into `linkmm` is good.

Steam launching a redirector that exits and leaves a detached game tree is bad.

Implication for `linkmm`:

- the host-side `linkmm` process must remain alive until the whole relevant game tree exits
- mount handle lifetime should be tied to that supervising process/session object

### 3. Flatpak Steam and filesystem access are real concerns

The MO2 installer has explicit docs and release changes around:

- Flatpak Steam filesystem access
- `STEAM_COMPAT_MOUNTS`
- letting Steam access manager files outside standard locations

Implication for `linkmm`:

- native `linkmm` should be treated as compatible only with native Steam
- Flatpak Steam should be treated as requiring a Flatpak build of `linkmm`
- do not rely on fragile host-to-Flatpak escape hatches as the primary supported path
- if `linkmm` binaries or helper files live outside Steam-visible paths, the design must either:
  - place the redirector/helper inside the game directory or compat-visible path, or
  - automatically require `STEAM_COMPAT_MOUNTS`/filesystem overrides for temporary or unsupported setups

Current product rule:

- native Steam -> native `linkmm`
- Flatpak Steam -> Flatpak `linkmm`

Packaging note:

- Flatpak packaging for `linkmm` may come later
- until that exists, Flatpak Steam users should be treated as unsupported for the Steam redirector flow

### 4. Script extenders are not optional for real users

For Skyrim, Fallout 4, Fallout 3, New Vegas, and Oblivion, users often do not want the stock launcher target. They want:

- `SKSE`
- `F4SE`
- `FOSE`
- `xNVSE`
- `OBSE`

Implication for `linkmm`:

- the redirector design cannot hardcode a single game EXE
- it needs per-game target selection with sane defaults

### 5. Launchers and child-process hops are normal

These games do not always keep the first EXE as the real long-lived process. Launchers, extenders, helper EXEs, crash reporters, and the actual game binary may all appear in sequence.

Implication for `linkmm`:

- first-child PID tracking is not enough
- process-group or descendant-tree tracking is required

## Proposed implementation plan

Before phased work begins, remove or de-prioritize code paths, UI text, and planning assumptions that imply broad multi-game support beyond the six-game set above.

## Phase 1 - Prove the model on Skyrim AE

Target only Skyrim AE/SE first.

Current status:

- [x] Added a dedicated headless Steam session mode: `linkmm --steam-session <appid>`
- [x] Restricted phase 1 to Skyrim SE / Anniversary Edition only
- [x] Enforced native-Steam-only support for the native `linkmm` build
- [x] Switched the phase-1 Steam game path off `umu-run` and onto a native Steam/Proton command
- [x] Added per-game Steam target selection for Skyrim (`launcher`, direct EXE, `SKSE`)
- [x] Added a generated Steam launch option in Preferences
- [x] Bound the generated Steam launch option to one exact configured game instance with `--game-id`
- [x] Added direct installation of the generated launch option into native Steam user config
- [x] Added installed-state detection and clear/remove support for native Steam launch options
- [x] Kept FUSE mount lifetime tied to the managed session and Unix process group
- [ ] Add the actual tiny redirector/helper layer if direct binary launch from Steam proves unreliable
- [ ] Verify real Skyrim SE / AE launch behavior under Steam with launcher target
- [ ] Verify real Skyrim SE / AE launch behavior under Steam with `SKSE`
- [ ] Verify Steam overlay / playtime / exit behavior on a real install
- [ ] Verify crash / forced-stop cleanup and final unmount timing

Deliverables:

- a Steam-launched redirector path that causes Steam to start `linkmm`
- `linkmm` mounts FUSE before launching the selected target
- selected target can be stock launcher/game or `SKSE`
- `linkmm` waits on the full descendant tree, not just the bootstrap child
- VFS unmount happens only after the full relevant tree exits

Why Skyrim first:

- strongest evidence base
- common script-extender workflow
- clean modern Bethesda baseline

## Phase 2 - Generalize to Fallout 4

Add Fallout 4 with the same architecture.

Deliverables:

- per-game target definitions
- `F4SE` target support
- compatibility test matrix for stock launcher versus extender launch

## Phase 3 - Add Fallout 3, New Vegas, Oblivion

Add older Bethesda titles after the core session model is proven.

Extra work expected:

- older launcher quirks
- New Vegas fullscreen caveat
- script extender naming/path differences
- more process-tree oddities

## Phase 4 - Decide Morrowind separately

Do not force Morrowind into the same milestone.

Decision gate:

- original Morrowind.exe under Proton redirector path
- or native/OpenMW support as a separate product path

## Technical design notes for linkmm

### A. Prefer "Steam launches a tiny stable redirector, redirector launches linkmm"

This is safer than trying to make Steam invoke the full `linkmm` binary path directly in every setup.

Reasons:

- easier to place next to the game or in a Steam-visible location
- smaller compatibility surface
- can pass game identity and target choice into `linkmm`
- easier Flatpak Steam handling

Needed behavior:

- redirector runs inside Steam's launch context
- redirector invokes `linkmm` with a `--steam-session` style mode
- redirector stays blocked until `linkmm` exits, or `exec`s into `linkmm`

### B. `linkmm` needs a dedicated Steam session mode

This mode should:

1. discover the Steam game/app context
2. resolve the configured target executable for this game
3. mount FUSE VFS
4. spawn the target inside the inherited Proton/Steam environment
5. supervise descendants
6. unmount and clean up after final exit

This mode should not behave like the generic current runtime path that was designed around `umu-run`.

### C. Store explicit per-game launch targets

Per game, configure:

- Steam app ID
- stock launcher EXE
- stock direct game EXE
- known script-extender EXE
- any skip-launcher arguments
- process names to consider "real game" or "keepalive descendants"

Minimal first set:

- Skyrim AE/SE: `SkyrimSELauncher.exe`, `SkyrimSE.exe`, `skse64_loader.exe`
- Fallout 4: `Fallout4Launcher.exe`, `Fallout4.exe`, `f4se_loader.exe`
- Fallout 3: `FalloutLauncherSteam.exe` or equivalent, `Fallout3.exe`, `fose_loader.exe`
- Fallout New Vegas: `FalloutNVLauncher.exe`, `FalloutNV.exe`, `nvse_loader.exe`
- Oblivion: `OblivionLauncher.exe`, `Oblivion.exe`, `obse_loader.exe`

Exact names should be verified against local installs or installer metadata before implementation is finalized.

### D. Descendant tracking must drive unmount

Unmount trigger should be:

- no tracked relevant descendant processes remain
- optional short grace period for launcher handoff races

Not acceptable:

- unmount when first child exits
- unmount when Steam changes visible status

### E. Flatpak Steam support must be part of the first design

Need one of these:

- install redirector/helper in a Steam-readable location
- or configure Steam launch options with `STEAM_COMPAT_MOUNTS=... %command%`
- or both

But the supported matrix should stay strict:

- native `linkmm` only promises support with native Steam
- Flatpak Steam support should be enabled only once `linkmm` itself is packaged for Flatpak

Ignoring this boundary would make the feature look flaky and produce hard-to-debug sandbox failures.

## Recommended first milestone

Build and test only this narrow set:

- Skyrim Anniversary Edition / Special Edition
- Fallout 4

Success criteria:

- launch from Steam library
- `linkmm` owns the FUSE mount for entire run
- script extender launch works
- Steam overlay/playtime/Steam ownership still work
- clean unmount on normal exit and on killed/crashed game

If those two work, the architecture is likely correct. Then add Fallout 3, New Vegas, and Oblivion. Leave Morrowind out until separate confirmation.

## Product direction change

This plan assumes `linkmm` should stop presenting itself as a general mod manager.

Practical consequences:

- remove roadmap items for unrelated games
- remove roadmap items for other Bethesda titles outside the six-game set
- avoid generic launcher support whose only purpose is future non-Bethesda expansion
- avoid abstractions that imply automatic expansion to all Bethesda titles
- prefer explicit per-game Bethesda handling over abstract "engine families" or "store-neutral" launch layers
- if existing code supports unrelated games, keep only what is cheap to retain and does not distort the new design

## Sources

- Furglitch/modorganizer2-linux-installer README and supported games table:
  https://github.com/Furglitch/modorganizer2-linux-installer
- Furglitch/modorganizer2-linux-installer post-install instructions:
  https://github.com/Furglitch/modorganizer2-linux-installer/wiki/Post%E2%80%90Install-Instructions
- DeepWiki summary of MO2 Linux installer's Steam integration and redirector behavior:
  https://deepwiki.com/rockerbacon/modorganizer2-linux-installer/3.2-launching-games-and-mo2
- DeepWiki supported games/app IDs overview:
  https://deepwiki.com/rockerbacon/modorganizer2-linux-installer/1.1-supported-games
- Steam community example for Fallout 3 keeping Steam features through MO2 launch:
  https://steamcommunity.com/app/22370/discussions/0/3467226415486012135/
- Skyrim Anniversary Edition on Steam, confirming AE is layered on Skyrim Special Edition app:
  https://store.steampowered.com/app/489830/The_Elder_Scrolls_V_Skyrim_Anniversary_Edition/
