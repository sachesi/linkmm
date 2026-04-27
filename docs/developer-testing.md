# Developer testing guide

## Test commands

Run core domain tests:

```bash
cargo test --lib
```

Run all tests (including UI):

```bash
cargo test
```

## Architecture Boundaries

- **Core (`src/core`)**: Contains the FUSE VFS, mod database management, Steam/UMU integration, and the runtime session manager. This layer should be kept independent of GTK.
- **UI (`src/ui`)**: GTK4/libadwaita components and views.

## VFS Testing

When testing changes to `src/core/vfs.rs`, ensure that:
- Case-insensitivity is maintained for all lookups.
- File priority is correctly handled (higher priority mods override lower ones).
- Writable overlay (CoW) correctly captures modifications in the scratch directory.

## Plugin / Load Order

- Plugin state is managed via `src/core/mods.rs`.
- `plugins.txt` is the source of truth for the game; LinkMM synchronizes state to this file before launch.
