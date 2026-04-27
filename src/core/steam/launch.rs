use std::path::{Path, PathBuf};

use super::library::{find_steam_root, is_path_in_flatpak};
use super::proton::find_proton_for_game;

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
