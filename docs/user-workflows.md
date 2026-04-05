# User workflows

## Profiles

1. Select active profile.
2. Enable/order mods for that profile.
3. Rebuild/deploy.
4. Switch profiles to move between isolated setups.

## Tool runs

1. Configure tool and run profile.
2. Launch tool from Tools page (Run).
3. LinkMM validates configuration.
4. While running, the launch action becomes Stop and output logs stream live.
5. On success, output is captured into managed generated package.
6. Deployment is rebuilt automatically.
7. If stopped by the user, the session is marked killed and UI unlocks when shutdown completes.

## Game sessions

1. Press Play for the active game instance.
2. Launch runs as a managed session (Steam or Non-Steam UMU backend based on instance source).
3. While active, Play becomes Stop for that instance.
4. Session state and live logs are shown in the main UI.
5. On exit (natural or stopped), session state finalizes and lock state clears.
6. On some Steam/Flatpak setups LinkMM tracks a delegated Steam session state
   (handoff acknowledged, best-effort stop) when a stable long-lived child process
   is not directly ownable.

## UI lock during active sessions

- When any managed game/tool session is active, LinkMM enters a controlled lock state.
- Game switching, profile switching, and navigation that could change deployment context are disabled.
- Stop actions and live logs remain accessible during lock state.

## Generated outputs

- View profile-scoped generated output packages in Tools page.
- Remove a package to undeploy its output and rebuild.
- Run stale cleanup for missing package sources.
- Adopt unmanaged files into managed packages when detection finds candidates.
