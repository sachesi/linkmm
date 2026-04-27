# User workflows

## Mod Management

1. **Install Mods:** Drag and drop archives or use the "Install" button to add mods to the library.
2. **Enable/Disable:** Use the toggle switch on each mod row.
3. **Ordering:** Drag and drop mods to change their priority. Higher priority mods (bottom of the list) override lower ones.
4. **Plugin Management:** View and sort your plugin load order in the "Load Order" tab.

## Launching the Game

1. **Automatic VFS:** Simply click "Play". LinkMM automatically mounts the FUSE VFS with your current mod selection.
2. **Launch:** The game starts via Proton/UMU in its dedicated prefix.
3. **Cleanup:** When the game exits, the VFS is unmounted automatically.

## Using External Tools

1. **Configuration:** Add tools (BodySlide, xEdit, etc.) in the "Tools" tab by selecting their executable.
2. **Launch:** Click "Run" on a tool. It launches with a writable VFS overlay.
3. **Output Capture:** Any files created or modified by the tool are captured in a scratch directory.
4. **Save Changes:** After the tool exits, a dialog appears asking if you want to keep the new files as a new mod entry.
