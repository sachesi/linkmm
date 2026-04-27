# linkmm Architecture

LinkMM uses a FUSE-based virtual filesystem (VFS) to manage Bethesda game mods on Linux.

## 1. VFS Model (FUSE)

Instead of symlinking or hardlinking files into the game directory, LinkMM mounts a union filesystem at launch.

- **Read-only union:** Merges the game's base `Data` directory with all enabled mods.
- **Priority-based:** Mods with higher priority (lower in the list) override files from lower-priority mods or the base game.
- **Case-insensitive:** Built-in support for case-insensitive lookups, as required by Bethesda engines.
- **Zero deployment time:** No files are moved or linked when enabling/disabling mods; the VFS is rebuilt instantly at launch.

## 2. Tool Sessions & Writable Overlay

External tools (BodySlide, Nemesis, etc.) often need to write new files into the `Data` directory.

- **Copy-on-Write (CoW):** When a tool is launched, the VFS includes a writable "upper" layer (a scratch directory). Any file modifications or new files are stored there.
- **Mod Creation:** After the tool exits, LinkMM detects changes in the scratch directory and offers to save them as a new mod.

## 3. Mod Database

The state is stored in `mods.toml` within the game's configuration folder.

- **Mod list:** Metadata about installed mods, their source paths, and enabled status.
- **Plugin order:** `plugins.txt` management and load order sorting.

## 4. Runtime Manager

`RuntimeSessionManager` handles the lifecycle of game and tool processes.

- **Sandboxing:** Commands are built using Proton (for Steam) or UMU (for non-Steam) to ensure they run in the correct Wine prefix.
- **VFS Lifecycle:** The VFS is mounted just before the process starts and is automatically unmounted when the process exits.
- **Log Streaming:** Stdout/stderr from the game or tool is captured and displayed in the UI.
