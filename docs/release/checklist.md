# Release Checklist

1. Confirm CI (`fmt`, `clippy`, `test`) is green on the release candidate.
2. Ensure `Cargo.toml` version matches the intended release version.
3. Update changelog/release notes.
4. Verify no secrets are exposed in logs or docs.
5. Tag release using `v<semver>` and publish artifacts.
