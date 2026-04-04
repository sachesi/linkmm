# Developer testing guide

## Test commands

Core/domain tests without UI feature:

```bash
cargo test --lib --no-default-features
```

UI + full app:

```bash
cargo test
```

## Domain vs UI boundaries

- Domain logic lives in `src/core`.
- UI state/actions live in `src/ui`.
- Add behavior tests in core modules whenever possible.

## Adding new tool adapters

1. Add adapter implementation in `src/core/tool_adapters.rs`.
2. Define defaults (`default_profiles`).
3. Add validation + classification + unmanaged detection.
4. Add tests for preset and validation behavior.

## Profile-scoped state rules

When changing domain logic, ensure changes are scoped to active profile:

- mod state
- plugin state
- generated outputs
- deployment state/backups/ownership

Never introduce global mutable behavior that bypasses active profile context.
