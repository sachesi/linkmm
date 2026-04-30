# LinkMM

LinkMM is a mod manager for Bethesda games (Skyrim, Fallout, etc.) designed for Linux. It uses a FUSE-based virtual filesystem to provide a "zero-deployment" experience.

## Key Features

- **Proton & UMU Integration**: Launch games and Windows-native tools using Steam's Proton or the Unified Moe Union (UMU) launcher.
- **Writable Overlay for Tools**: Run tools like BodySlide, Nemesis, or xEdit directly against your modded setup. Changes are captured and can be saved as new mods.
- **Native GTK4 UI**: A clean interface built with Libadwaita that follows GNOME HIG.
- **Nexus Mods Integration**: Handle `nxm://` links and manage your Nexus downloads directly.

## Documentation

- [Architecture Overview](docs/architecture.md) — How the VFS and runtime work.
- [User Workflows](docs/user-workflows.md) — How to manage mods and tools.
- [Developer Guide](docs/developer-testing.md) — Testing and development boundaries.

## Getting Started

### Prerequisites

- `rustc` and `cargo` (latest stable)
- `libadwaita` and `gtk4` development headers
- `libfuse3` (required for the VFS)

### Running

```bash
cargo run
```

### Testing

```bash
cargo test
```

## Project Structure

- `src/core/`: The "brains" of the app. Handles VFS, game detection, and process management.
- `src/ui/`: The user interface components.
- `docs/`: Technical and user documentation.

---

Built with ❤️ and help from Gemini
