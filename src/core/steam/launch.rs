use std::path::{Path, PathBuf};

use super::library::{find_steam_root, is_path_in_flatpak, is_steam_flatpak};
use super::proton::find_proton_for_game;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ManagedSteamBackend {
    Native,
    Flatpak,
    XdgOpenFallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SteamPathKind {
    Native,
    FlatpakWrapper,
    Missing,
}

/// Launch a game through the Steam client via `xdg-open steam://run/<app_id>`.
pub fn launch_game(game: &crate::core::games::Game) -> Result<(), String> {
    let app_id = game
        .steam_instance_app_id()
        .ok_or_else(|| "Game has no Steam App ID".to_string())?;

    std::process::Command::new("xdg-open")
        .arg(format!("steam://run/{app_id}"))
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Failed to open steam://run/{app_id}: {e}"))
}

/// Build a managed Steam launch command using `steam -applaunch` or the Flatpak equivalent.
pub fn launch_game_managed_command(
    game: &crate::core::games::Game,
) -> Result<std::process::Command, String> {
    let app_id = game
        .steam_instance_app_id()
        .ok_or_else(|| "Game has no Steam App ID".to_string())?;
    let command = match select_managed_steam_backend(is_steam_flatpak(), detect_steam_path_kind()) {
        ManagedSteamBackend::Flatpak => launch_game_managed_flatpak_command(app_id),
        ManagedSteamBackend::Native => launch_game_managed_native_command(app_id),
        ManagedSteamBackend::XdgOpenFallback => {
            let mut command = std::process::Command::new("xdg-open");
            command.arg(format!("steam://run/{app_id}"));
            command
        }
    };
    Ok(command)
}

pub(super) fn select_managed_steam_backend(
    is_flatpak: bool,
    path_kind: SteamPathKind,
) -> ManagedSteamBackend {
    if is_flatpak {
        ManagedSteamBackend::Flatpak
    } else {
        match path_kind {
            SteamPathKind::Native => ManagedSteamBackend::Native,
            SteamPathKind::FlatpakWrapper => ManagedSteamBackend::Flatpak,
            SteamPathKind::Missing => ManagedSteamBackend::XdgOpenFallback,
        }
    }
}

pub(super) fn detect_steam_path_kind() -> SteamPathKind {
    let Some(path_env) = std::env::var_os("PATH") else {
        return SteamPathKind::Missing;
    };
    for dir in std::env::split_paths(&path_env) {
        let steam = dir.join("steam");
        if !steam.is_file() {
            continue;
        }
        let canonical = std::fs::canonicalize(&steam).unwrap_or(steam.clone());
        let canonical_str = canonical.to_string_lossy().to_lowercase();
        if canonical_str.contains("flatpak") {
            return SteamPathKind::FlatpakWrapper;
        }
        if let Ok(contents) = std::fs::read_to_string(&steam)
            && contents.contains("com.valvesoftware.Steam")
        {
            return SteamPathKind::FlatpakWrapper;
        }
        return SteamPathKind::Native;
    }
    SteamPathKind::Missing
}

fn launch_game_managed_native_command(app_id: u32) -> std::process::Command {
    let mut command = std::process::Command::new("steam");
    command.arg("-applaunch").arg(app_id.to_string());
    command
}

fn launch_game_managed_flatpak_command(app_id: u32) -> std::process::Command {
    let mut command = std::process::Command::new("flatpak");
    command
        .arg("run")
        .arg("com.valvesoftware.Steam")
        .arg("-applaunch")
        .arg(app_id.to_string());
    command
}

/// Launch an external tool through Proton, automatically selecting native or Flatpak backend.
pub fn launch_tool_with_proton(
    exe_path: &PathBuf,
    arguments: &str,
    app_id: u32,
) -> Result<std::process::Child, String> {
    let (proton_path, compatdata_path) = find_proton_for_game(app_id)?;
    let proton_script = proton_path.join("proton");

    if !proton_script.exists() {
        return Err(format!("Proton script not found at {}", proton_script.display()));
    }
    if !exe_path.exists() {
        return Err(format!("Executable not found at {}", exe_path.display()));
    }

    let steam_root =
        find_steam_root().ok_or_else(|| "Could not find Steam installation".to_string())?;

    log::info!(
        "Launching tool {} with Proton from {}",
        exe_path.display(),
        proton_path.display()
    );
    log::debug!("Using compatdata: {}", compatdata_path.display());

    let is_flatpak = is_path_in_flatpak(&proton_path) || is_path_in_flatpak(&compatdata_path);

    if is_flatpak {
        log::info!("Detected Flatpak Steam, using flatpak wrapper");
        let mut command = build_flatpak_tool_command(
            &proton_script,
            exe_path,
            arguments,
            &steam_root,
            &compatdata_path,
            app_id,
        )?;
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        log::debug!("Executing flatpak command: {:?}", command);
        command.spawn().map_err(|e| format!("Failed to spawn Flatpak process: {e}"))
    } else {
        log::debug!("Using native Steam launch");
        let mut command = std::process::Command::new(&proton_script);
        command.env("STEAM_COMPAT_DATA_PATH", &compatdata_path);
        command.env("STEAM_COMPAT_CLIENT_INSTALL_PATH", &steam_root);
        command.env("SteamAppId", app_id.to_string());
        command.env("SteamGameId", app_id.to_string());
        command.arg("run");
        command.arg(exe_path);
        for arg in split_launch_arguments(arguments)? {
            command.arg(arg);
        }
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        log::debug!("Executing: {:?}", command);
        command.spawn().map_err(|e| format!("Failed to spawn Proton process: {e}"))
    }
}

/// Build a tool launch command without spawning (useful for testing or managed sessions).
pub fn build_tool_command(
    exe_path: &PathBuf,
    arguments: &str,
    app_id: u32,
) -> Result<std::process::Command, String> {
    let (proton_path, compatdata_path) = find_proton_for_game(app_id)?;
    let proton_script = proton_path.join("proton");
    if !proton_script.exists() {
        return Err(format!("Proton script not found at {}", proton_script.display()));
    }
    if !exe_path.exists() {
        return Err(format!("Executable not found at {}", exe_path.display()));
    }
    let steam_root =
        find_steam_root().ok_or_else(|| "Could not find Steam installation".to_string())?;
    let is_flatpak = is_path_in_flatpak(&proton_path) || is_path_in_flatpak(&compatdata_path);
    if is_flatpak {
        build_flatpak_tool_command(
            &proton_script,
            exe_path,
            arguments,
            &steam_root,
            &compatdata_path,
            app_id,
        )
    } else {
        build_native_tool_command(
            &proton_script,
            exe_path,
            arguments,
            &steam_root,
            &compatdata_path,
            app_id,
        )
    }
}

pub(super) fn build_native_tool_command(
    proton_script: &PathBuf,
    exe_path: &PathBuf,
    arguments: &str,
    steam_root: &PathBuf,
    compatdata_path: &PathBuf,
    app_id: u32,
) -> Result<std::process::Command, String> {
    let mut command = std::process::Command::new(proton_script);
    command.env("STEAM_COMPAT_DATA_PATH", compatdata_path);
    command.env("STEAM_COMPAT_CLIENT_INSTALL_PATH", steam_root);
    command.env("SteamAppId", app_id.to_string());
    command.env("SteamGameId", app_id.to_string());
    command.env("STEAM_APPID", app_id.to_string());
    command.arg("run");
    command.arg(exe_path);
    for arg in split_launch_arguments(arguments)? {
        command.arg(arg);
    }
    if let Some(dir) = exe_path.parent() {
        command.current_dir(dir);
    }
    Ok(command)
}

fn build_flatpak_tool_command(
    proton_script: &Path,
    exe_path: &Path,
    arguments: &str,
    steam_root: &Path,
    compatdata_path: &Path,
    app_id: u32,
) -> Result<std::process::Command, String> {
    let mut command = std::process::Command::new("flatpak");
    command
        .arg("run")
        .arg(format!("--env=STEAM_COMPAT_CLIENT_INSTALL_PATH={}", steam_root.display()))
        .arg(format!("--env=STEAM_COMPAT_DATA_PATH={}", compatdata_path.display()))
        .arg(format!("--env=SteamAppId={app_id}"))
        .arg(format!("--env=SteamGameId={app_id}"))
        .arg(format!("--env=STEAM_APPID={app_id}"))
        .arg(format!("--command={}", proton_script.display()))
        .arg("com.valvesoftware.Steam")
        .arg("run")
        .arg(exe_path);
    for arg in split_launch_arguments(arguments)? {
        command.arg(arg);
    }
    if let Some(dir) = exe_path.parent() {
        command.arg(format!("--cwd={}", dir.display()));
    }
    Ok(command)
}

pub fn split_launch_arguments(arguments: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in arguments.chars() {
        if escaped {
            current.push(c);
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '\'' | '"' => {
                if quote == Some(c) {
                    quote = None;
                } else if quote.is_none() {
                    quote = Some(c);
                } else {
                    current.push(c);
                }
            }
            c if c.is_whitespace() && quote.is_none() => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(c),
        }
    }
    if escaped {
        current.push('\\');
    }
    if quote.is_some() {
        return Err("Tool arguments contain an unmatched quote".to_string());
    }
    if !current.is_empty() {
        out.push(current);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn launch_game_fails_without_steam_app_id() {
        use crate::core::games::{Game, GameKind};
        let game = Game::new_steam(GameKind::SkyrimSE, std::path::PathBuf::from("/fake"));
        match launch_game(&game) {
            Ok(_) => {}
            Err(e) => assert!(
                !e.contains("Steam App ID"),
                "error should not be about missing App ID for SkyrimSE: {e}"
            ),
        }
    }

    #[test]
    fn managed_backend_selection_prefers_flatpak_when_detected() {
        assert_eq!(
            select_managed_steam_backend(true, SteamPathKind::Native),
            ManagedSteamBackend::Flatpak
        );
        assert_eq!(
            select_managed_steam_backend(true, SteamPathKind::Missing),
            ManagedSteamBackend::Flatpak
        );
    }

    #[test]
    fn managed_backend_selection_uses_native_when_available() {
        assert_eq!(
            select_managed_steam_backend(false, SteamPathKind::Native),
            ManagedSteamBackend::Native
        );
    }

    #[test]
    fn managed_backend_selection_uses_xdg_open_only_as_last_resort() {
        assert_eq!(
            select_managed_steam_backend(false, SteamPathKind::Missing),
            ManagedSteamBackend::XdgOpenFallback
        );
    }

    #[test]
    fn managed_backend_selection_uses_flatpak_for_flatpak_wrapper_on_path() {
        assert_eq!(
            select_managed_steam_backend(false, SteamPathKind::FlatpakWrapper),
            ManagedSteamBackend::Flatpak
        );
    }

    #[test]
    fn steam_launch_command_uses_instance_app_id() {
        use crate::core::games::{Game, GameKind};
        let game = Game::new_steam_with_app_id(GameKind::FalloutNV, PathBuf::from("/fake"), 22490);
        let command = launch_game_managed_command(&game).expect("managed launch command");
        let args: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        let has_22490 = args
            .iter()
            .any(|a| a == "22490" || a.contains("steam://run/22490"));
        assert!(
            has_22490,
            "expected launch command args to include app id 22490: {args:?}"
        );
    }

    #[test]
    fn native_tool_command_uses_supplied_instance_app_id() {
        let cmd_pcr = build_native_tool_command(
            &PathBuf::from("/proton/proton"),
            &PathBuf::from("/games/FalloutNV.exe"),
            "",
            &PathBuf::from("/steam/root"),
            &PathBuf::from("/steamapps/compatdata/22490"),
            22490,
        )
        .expect("pcr command");
        assert_eq!(
            cmd_pcr.get_envs().find(|(k, _)| *k == "SteamAppId"),
            Some((
                std::ffi::OsStr::new("SteamAppId"),
                Some(std::ffi::OsStr::new("22490"))
            ))
        );

        let cmd_base = build_native_tool_command(
            &PathBuf::from("/proton/proton"),
            &PathBuf::from("/games/FalloutNV.exe"),
            "",
            &PathBuf::from("/steam/root"),
            &PathBuf::from("/steamapps/compatdata/22380"),
            22380,
        )
        .expect("base command");
        assert_eq!(
            cmd_base.get_envs().find(|(k, _)| *k == "SteamAppId"),
            Some((
                std::ffi::OsStr::new("SteamAppId"),
                Some(std::ffi::OsStr::new("22380"))
            ))
        );
    }

    #[test]
    fn flatpak_managed_game_command_uses_flatpak_run_applaunch() {
        let command = launch_game_managed_flatpak_command(489830);
        let program = command.get_program().to_string_lossy().to_string();
        let args: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(program, "flatpak");
        assert_eq!(
            args,
            vec!["run", "com.valvesoftware.Steam", "-applaunch", "489830"]
        );
    }

    #[test]
    fn native_managed_game_command_uses_steam_applaunch() {
        let command = launch_game_managed_native_command(489830);
        let program = command.get_program().to_string_lossy().to_string();
        let args: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(program, "steam");
        assert_eq!(args, vec!["-applaunch", "489830"]);
    }

    #[test]
    fn split_launch_arguments_preserves_quoted_spaces() {
        let args = split_launch_arguments(r#"-flag "value one" --path '/tmp/tool dir'"#).unwrap();
        assert_eq!(args, vec!["-flag", "value one", "--path", "/tmp/tool dir"]);
    }

    #[test]
    fn build_flatpak_tool_command_handles_spaces_without_shell_concat() {
        let cmd = build_flatpak_tool_command(
            Path::new("/flatpak/proton/proton"),
            Path::new("/games/My Tool.exe"),
            r#"--profile "Default Profile" --output "out dir""#,
            Path::new("/flatpak/steam/root"),
            Path::new("/flatpak/compatdata/489830"),
            489830,
        )
        .unwrap();
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert!(args.iter().any(|a| a == "--profile"));
        assert!(args.iter().any(|a| a == "Default Profile"));
        assert!(args.iter().any(|a| a == "out dir"));
        assert!(args.iter().any(|a| a.starts_with("--env=SteamAppId=489830")));
        assert!(args.iter().any(|a| a.starts_with("--env=SteamGameId=489830")));
    }
}
