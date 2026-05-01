use crate::core::config::{AppConfig, ToolConfig};
use crate::core::games::{Game, GameLauncherSource};
use crate::core::mods::ModDatabase;
use crate::core::vfs::{MountHandle, mount_mod_vfs, mount_tool_vfs};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant, SystemTime};

const MAX_LOG_LINES: usize = 600;

#[cfg(unix)]
static STEAM_SESSION_STOP: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn steam_session_term_handler(_: libc::c_int) {
    STEAM_SESSION_STOP.store(true, Ordering::SeqCst);
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SessionKind {
    Game,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SessionStatus {
    Starting,
    Running,
    Exited,
    Failed,
    Killed,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SessionStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SessionRunMode {
    FullyManaged,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionLogLine {
    pub stream: SessionStream,
    pub at: SystemTime,
    pub message: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ManagedSession {
    pub id: u64,
    pub kind: SessionKind,
    pub game_id: String,
    pub tool_id: Option<String>,
    pub launcher_source: GameLauncherSource,
    pub pid: Option<u32>,
    pub started_at: SystemTime,
    pub status: SessionStatus,
    pub exit_code: Option<i32>,
    pub run_mode: SessionRunMode,
    /// Writable overlay staging directory for tool sessions (None for game sessions).
    pub scratch_dir: Option<PathBuf>,
}

struct SessionControl {
    child: Mutex<Option<Child>>,
    stop_requested: AtomicBool,
}

struct SessionRecord {
    session: ManagedSession,
    logs: VecDeque<SessionLogLine>,
    control: Arc<SessionControl>,
}

type SessionCompletion = Box<dyn FnOnce(ExitStatus) -> Result<(), String> + Send + 'static>;

struct SessionStart {
    kind: SessionKind,
    game_id: String,
    tool_id: Option<String>,
    launcher_source: GameLauncherSource,
    command: std::process::Command,
    completion: Option<SessionCompletion>,
    vfs_mount: Option<MountHandle>,
}

#[derive(Clone)]
pub struct RuntimeSessionManager {
    inner: Arc<RuntimeSessionManagerInner>,
}

struct RuntimeSessionManagerInner {
    next_id: AtomicU64,
    sessions: Mutex<HashMap<u64, SessionRecord>>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
struct SessionRegistry {
    sessions: Vec<ManagedSession>,
}

fn session_registry_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("linkmm")
        .join("sessions.toml")
}

impl RuntimeSessionManager {
    fn save_sessions(&self) {
        let active_sessions: Vec<ManagedSession> = self
            .inner
            .sessions
            .lock()
            .expect("lock")
            .values()
            .map(|s| s.session.clone())
            .filter(|s| matches!(s.status, SessionStatus::Starting | SessionStatus::Running))
            .collect();

        let registry = SessionRegistry {
            sessions: active_sessions,
        };
        if let Ok(content) = toml::to_string_pretty(&registry) {
            let path = session_registry_path();
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(path, content);
        }
    }
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RuntimeSessionManagerInner {
                next_id: AtomicU64::new(1),
                sessions: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn any_active(&self) -> bool {
        self.sessions()
            .into_iter()
            .any(|s| matches!(s.status, SessionStatus::Starting | SessionStatus::Running))
    }

    pub fn sessions(&self) -> Vec<ManagedSession> {
        let mut out: Vec<ManagedSession> = self
            .inner
            .sessions
            .lock()
            .expect("sessions lock poisoned")
            .values()
            .map(|s| s.session.clone())
            .collect();
        out.sort_by_key(|s| s.id);
        out
    }

    pub fn session_logs(&self, id: u64) -> Vec<SessionLogLine> {
        self.inner
            .sessions
            .lock()
            .expect("sessions lock poisoned")
            .get(&id)
            .map(|s| s.logs.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn current_game_session(&self, game_id: &str) -> Option<ManagedSession> {
        self.sessions().into_iter().find(|s| {
            s.kind == SessionKind::Game
                && s.game_id == game_id
                && matches!(s.status, SessionStatus::Starting | SessionStatus::Running)
        })
    }

    pub fn current_tool_session(&self, game_id: &str, tool_id: &str) -> Option<ManagedSession> {
        self.sessions().into_iter().find(|s| {
            s.kind == SessionKind::Tool
                && s.game_id == game_id
                && s.tool_id.as_deref() == Some(tool_id)
                && matches!(s.status, SessionStatus::Starting | SessionStatus::Running)
        })
    }

    pub fn stop_session(&self, id: u64) -> Result<(), String> {
        let control = {
            let sessions = self.inner.sessions.lock().expect("sessions lock poisoned");
            sessions
                .get(&id)
                .map(|s| Arc::clone(&s.control))
                .ok_or_else(|| format!("Session {id} was not found"))?
        };
        control.stop_requested.store(true, Ordering::SeqCst);
        let mut child_guard = control.child.lock().expect("child lock poisoned");
        if let Some(child) = child_guard.as_mut() {
            #[cfg(unix)]
            {
                let pid = child.id();
                // Send SIGTERM to the process group (negative PID)
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGTERM);
                }
            }
            #[cfg(not(unix))]
            {
                child
                    .kill()
                    .map_err(|e| format!("Failed to stop session {id}: {e}"))?;
            }
        }
        Ok(())
    }

    pub fn start_game_session(&self, game: Game) -> Result<u64, String> {
        if self.current_game_session(&game.id).is_some() {
            return Err("A game session is already active for this game instance".to_string());
        }
        if game.launcher_source == GameLauncherSource::Steam
            && !game.kind.is_phase1_steam_redirector_target()
        {
            return Err(format!(
                "Phase 1 Steam session support is currently limited to Skyrim SE/AE. {} is not enabled yet.",
                game.kind.display_name()
            ));
        }

        let db = ModDatabase::load(&game);
        if let Err(e) = db.write_plugins_txt(&game) {
            log::warn!("Failed to write plugins.txt before game launch: {e}");
        }
        let vfs_mount =
            mount_mod_vfs(&game, &db).map_err(|e| format!("Failed to mount mod VFS: {e}"))?;

        let command = match game.launcher_source {
            GameLauncherSource::NonSteamUmu => {
                if !crate::core::umu::is_umu_available() {
                    return Err(
                        "umu-launcher is not installed. Please try again or check settings."
                            .to_string(),
                    );
                }
                let umu = game.validate_umu_setup()?;
                let app_id = game.kind.primary_steam_app_id().unwrap_or(0);
                crate::core::umu::build_umu_command(
                    &umu.exe_path,
                    "",
                    app_id,
                    umu.prefix_path.as_deref(),
                    umu.proton_path.as_deref(),
                    "none",
                    None,
                )?
            }
            GameLauncherSource::Steam => {
                let app_id = game.steam_instance_app_id().unwrap_or(0);
                let preferred_exe = AppConfig::load_or_default()
                    .game_settings_ref(&game.id)
                    .and_then(|settings| settings.steam_redirect_exe.clone());
                let exe_path = resolve_phase1_steam_exe_path(&game, preferred_exe.as_deref())?;
                crate::core::steam::build_game_command(&exe_path, app_id)?
            }
        };

        self.spawn_managed_session(
            SessionStart {
                kind: SessionKind::Game,
                game_id: game.id,
                tool_id: None,
                launcher_source: game.launcher_source,
                command,
                completion: None,
                vfs_mount: Some(vfs_mount),
            },
            None,
        )
        .map(|(id, _)| id)
    }

    pub fn start_tool_session(
        &self,
        game: Game,
        tool: ToolConfig,
    ) -> Result<(u64, mpsc::Receiver<Result<(), String>>), String> {
        if self.current_tool_session(&game.id, &tool.id).is_some() {
            return Err(format!("Tool {} is already running", tool.name));
        }

        let (done_tx, done_rx) = mpsc::channel::<Result<(), String>>();

        let completion: SessionCompletion = Box::new(move |status: ExitStatus| {
            let result = if status.success() {
                Ok(())
            } else {
                Err(format!("Tool exited with non-zero status: {status}"))
            };
            let _ = done_tx.send(result.clone());
            result
        });

        let command = match game.launcher_source {
            GameLauncherSource::NonSteamUmu => {
                let umu = game.validate_umu_setup()?;
                // Use the game's Steam App ID for GAMEID so umu-run applies the
                // correct protonfixes for the game, not for the tool's (possibly
                // unconfigured) app_id.
                let app_id = game.kind.primary_steam_app_id().unwrap_or(tool.app_id);
                crate::core::umu::build_umu_command(
                    &tool.exe_path,
                    &tool.arguments,
                    app_id,
                    umu.prefix_path.as_deref(),
                    umu.proton_path.as_deref(),
                    "none",
                    None,
                )?
            }
            GameLauncherSource::Steam => {
                let app_id = game.steam_instance_app_id().unwrap_or(tool.app_id);
                let (proton_path, compatdata_path) =
                    crate::core::steam::proton::find_proton_for_game(app_id)?;
                let prefix_path = compatdata_path.join("pfx");

                crate::core::umu::build_umu_command(
                    &tool.exe_path,
                    &tool.arguments,
                    app_id,
                    Some(&prefix_path),
                    Some(&proton_path),
                    "steam",
                    crate::core::steam::library::find_steam_root().as_deref(),
                )?
            }
        };

        let db = ModDatabase::load(&game);
        // Use writable overlay VFS for tool sessions so tools can write output files
        let vfs_mount = mount_tool_vfs(&game, &db, &tool.id)
            .map_err(|e| format!("Failed to mount tool VFS: {e}"))?;

        let scratch_dir = vfs_mount.writable_upper.clone();
        let id = self
            .spawn_managed_session(
                SessionStart {
                    kind: SessionKind::Tool,
                    game_id: game.id,
                    tool_id: Some(tool.id),
                    launcher_source: game.launcher_source,
                    command,
                    completion: Some(completion),
                    vfs_mount: Some(vfs_mount),
                },
                scratch_dir,
            )
            .map(|(id, _)| id)?;

        Ok((id, done_rx))
    }

    fn spawn_managed_session(
        &self,
        mut start: SessionStart,
        scratch_dir: Option<PathBuf>,
    ) -> Result<(u64, Arc<SessionControl>), String> {
        start.command.stdout(Stdio::piped());
        start.command.stderr(Stdio::piped());

        #[cfg(unix)]
        {
            start.command.process_group(0);
        }

        let mut child = start
            .command
            .spawn()
            .map_err(|e| format!("Failed to spawn managed session: {e}"))?;

        let pid = Some(child.id());
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        let control = Arc::new(SessionControl {
            child: Mutex::new(Some(child)),
            stop_requested: AtomicBool::new(false),
        });

        {
            let mut sessions = self.inner.sessions.lock().expect("sessions lock poisoned");
            sessions.insert(
                id,
                SessionRecord {
                    session: ManagedSession {
                        id,
                        kind: start.kind,
                        game_id: start.game_id,
                        tool_id: start.tool_id,
                        launcher_source: start.launcher_source,
                        pid,
                        started_at: SystemTime::now(),
                        status: SessionStatus::Running,
                        exit_code: None,
                        run_mode: SessionRunMode::FullyManaged,
                        scratch_dir,
                    },
                    logs: VecDeque::new(),
                    control: Arc::clone(&control),
                },
            );
        }

        if let Some(stdout) = stdout {
            self.spawn_log_reader(id, SessionStream::Stdout, stdout);
        }
        if let Some(stderr) = stderr {
            self.spawn_log_reader(id, SessionStream::Stderr, stderr);
        }

        let manager = self.clone();
        let control_for_wait = Arc::clone(&control);
        let vfs_mount = start.vfs_mount;
        std::thread::spawn(move || {
            let status_result = {
                let mut child_guard = control_for_wait.child.lock().expect("child lock poisoned");
                child_guard
                    .take()
                    .ok_or_else(|| "Session process handle missing".to_string())
                    .and_then(|mut child| {
                        let child_pid = child.id();
                        child
                            .wait()
                            .map_err(|e| format!("Failed waiting for managed session: {e}"))
                            .and_then(|status| {
                                #[cfg(unix)]
                                wait_for_process_group_exit(
                                    child_pid,
                                    control_for_wait.stop_requested.load(Ordering::SeqCst),
                                )?;
                                Ok(status)
                            })
                    })
            };

            // Signal the VFS that the session has ended (stops accepting writes)
            if let Some(ref mount) = vfs_mount {
                mount.session_ended.store(true, Ordering::SeqCst);
            }

            manager.finish_session(
                id,
                status_result,
                start.completion,
                control_for_wait.stop_requested.load(Ordering::SeqCst),
            );
            // VFS mount is dropped here, unmounting after process exits.
            drop(vfs_mount);
        });

        self.save_sessions();
        Ok((id, control))
    }

    fn finish_session(
        &self,
        id: u64,
        status_result: Result<ExitStatus, String>,
        completion: Option<SessionCompletion>,
        stop_requested: bool,
    ) {
        let mut next_status = SessionStatus::Failed;
        let mut exit_code = None;

        match status_result {
            Ok(status) => {
                exit_code = status.code();
                let completion_result: Result<(), String> =
                    completion.map(|done| done(status)).unwrap_or(Ok(()));
                next_status = if completion_result.is_err() {
                    SessionStatus::Failed
                } else if stop_requested {
                    SessionStatus::Killed
                } else if status.success() {
                    SessionStatus::Exited
                } else {
                    SessionStatus::Failed
                };
            }
            Err(e) => {
                self.push_log(id, SessionStream::Stderr, e.to_string());
            }
        }

        let mut sessions = self.inner.sessions.lock().expect("sessions lock poisoned");
        if let Some(record) = sessions.get_mut(&id) {
            record.session.status = next_status;
            record.session.exit_code = exit_code;
        }
        drop(sessions);
        self.save_sessions();
    }

    fn spawn_log_reader<R: std::io::Read + Send + 'static>(
        &self,
        id: u64,
        stream: SessionStream,
        reader: R,
    ) {
        let manager = self.clone();
        std::thread::spawn(move || {
            let buf_reader = BufReader::new(reader);
            for line in buf_reader.lines().map_while(Result::ok) {
                match stream {
                    SessionStream::Stdout => log::info!("[session:{id}] {line}"),
                    SessionStream::Stderr => log::warn!("[session:{id}] {line}"),
                }
                manager.push_log(id, stream.clone(), line);
            }
        });
    }

    fn push_log(&self, id: u64, stream: SessionStream, message: String) {
        let mut sessions = self.inner.sessions.lock().expect("sessions lock poisoned");
        if let Some(record) = sessions.get_mut(&id) {
            record.logs.push_back(SessionLogLine {
                stream,
                at: SystemTime::now(),
                message,
            });
            while record.logs.len() > MAX_LOG_LINES {
                record.logs.pop_front();
            }
        }
    }
}

#[cfg(unix)]
fn wait_for_process_group_exit(pgid: u32, stop_requested: bool) -> Result<(), String> {
    let start = Instant::now();
    let mut sigkill_sent = false;

    while process_group_exists(pgid) {
        if stop_requested && !sigkill_sent && start.elapsed() >= Duration::from_secs(5) {
            unsafe {
                libc::kill(-(pgid as i32), libc::SIGKILL);
            }
            sigkill_sent = true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    Ok(())
}

#[cfg(unix)]
fn process_group_exists(pgid: u32) -> bool {
    unsafe { libc::kill(-(pgid as i32), 0) == 0 }
}

fn resolve_phase1_steam_exe_path(
    game: &Game,
    preferred_exe: Option<&str>,
) -> Result<PathBuf, String> {
    if let Some(preferred) = preferred_exe {
        let preferred_path = game.root_path.join(preferred);
        if preferred_path.exists() {
            return Ok(preferred_path);
        }
    }

    for exe in game.kind.phase1_steam_launch_candidates() {
        let path = game.root_path.join(exe);
        if path.exists() {
            return Ok(path);
        }
    }

    Err("Could not find game executable in root path".to_string())
}

fn resolve_phase1_steam_game(
    config: &AppConfig,
    app_id: u32,
    requested_game_id: Option<&str>,
) -> Result<Game, String> {
    let mut matches: Vec<Game> = config
        .games
        .iter()
        .filter(|game| {
            game.launcher_source == GameLauncherSource::Steam
                && game.steam_instance_app_id() == Some(app_id)
                && game.kind.is_phase1_steam_redirector_target()
        })
        .cloned()
        .collect();

    if matches.is_empty() {
        return Err(format!(
            "No configured phase-1 Steam game instance found for app id {app_id}."
        ));
    }

    if let Some(requested_game_id) = requested_game_id {
        return matches
            .into_iter()
            .find(|game| game.id == requested_game_id)
            .ok_or_else(|| {
                format!(
                    "Configured game id {requested_game_id} does not match any phase-1 Steam game instance for app id {app_id}."
                )
            });
    }

    if let Some(current_id) = config.current_game_id.as_deref()
        && let Some(game) = matches.iter().find(|game| game.id == current_id)
    {
        return Ok(game.clone());
    }

    if let Ok(current_dir) = std::env::current_dir()
        && let Some(game) = matches.iter().find(|game| game.root_path == current_dir)
    {
        return Ok(game.clone());
    }

    if matches.len() == 1 {
        return Ok(matches.remove(0));
    }

    Err(format!(
        "Multiple configured phase-1 Steam game instances match app id {app_id}. Use a launch option generated for one exact configured game instance."
    ))
}

struct SessionLock {
    path: PathBuf,
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn acquire_steam_session_lock(app_id: u32) -> Result<SessionLock, String> {
    let lock_dir = dirs::runtime_dir()
        .or_else(dirs::data_local_dir)
        .ok_or_else(|| "Could not determine runtime directory for session lock".to_string())?
        .join("linkmm");

    std::fs::create_dir_all(&lock_dir)
        .map_err(|e| format!("Failed to create session lock directory: {e}"))?;

    let lock_path = lock_dir.join(format!("steam-session-{app_id}.pid"));

    // Clean up stale lock from a prior crash before trying to create.
    if let Ok(contents) = std::fs::read_to_string(&lock_path) {
        let stale = contents
            .trim()
            .parse::<u32>()
            .map(|pid| {
                #[cfg(unix)]
                {
                    unsafe { libc::kill(pid as libc::pid_t, 0) != 0 }
                }
                #[cfg(not(unix))]
                {
                    let _ = pid;
                    false
                }
            })
            .unwrap_or(true);
        if stale {
            let _ = std::fs::remove_file(&lock_path);
        }
    }

    // O_CREAT | O_EXCL — fails if another live instance already holds the lock.
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
    {
        Ok(mut file) => {
            use std::io::Write;
            write!(file, "{}", std::process::id())
                .map_err(|e| format!("Failed to write session lock: {e}"))?;
            Ok(SessionLock { path: lock_path })
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(format!(
            "Steam session for app {app_id} is already running. \
             Stop the existing session before launching again."
        )),
        Err(e) => Err(format!("Failed to acquire session lock: {e}")),
    }
}

pub fn run_phase1_steam_session(
    app_id: u32,
    requested_game_id: Option<&str>,
) -> Result<i32, String> {
    let _lock = acquire_steam_session_lock(app_id)?;

    let mut config = AppConfig::load_or_default();
    config.apply_mods_base_dirs();
    let game = resolve_phase1_steam_game(&config, app_id, requested_game_id)?;
    let manager = global_runtime_manager();
    let id = manager.start_game_session(game)?;

    #[cfg(unix)]
    {
        STEAM_SESSION_STOP.store(false, Ordering::SeqCst);
        unsafe {
            libc::signal(libc::SIGTERM, steam_session_term_handler as libc::sighandler_t);
            libc::signal(libc::SIGINT, steam_session_term_handler as libc::sighandler_t);
        }
    }

    let mut stop_forwarded = false;
    loop {
        #[cfg(unix)]
        if !stop_forwarded && STEAM_SESSION_STOP.load(Ordering::SeqCst) {
            stop_forwarded = true;
            let _ = manager.stop_session(id);
        }

        let session = manager
            .sessions()
            .into_iter()
            .find(|session| session.id == id)
            .ok_or_else(|| format!("Managed session {id} disappeared unexpectedly"))?;

        match session.status {
            SessionStatus::Starting | SessionStatus::Running => {
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            SessionStatus::Exited => return Ok(session.exit_code.unwrap_or(0)),
            SessionStatus::Killed => return Ok(session.exit_code.unwrap_or(130)),
            SessionStatus::Failed => {
                return Err(format!(
                    "Steam session failed{}",
                    session
                        .exit_code
                        .map(|code| format!(" with exit code {code}"))
                        .unwrap_or_default()
                ));
            }
        }
    }
}

pub fn build_phase1_steam_launch_option(app_id: u32, game_id: &str) -> Result<String, String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("Failed to locate current linkmm binary: {e}"))?;
    Ok(format!(
        "{} --steam-session {app_id} --game-id {}",
        shell_quote_path(&exe),
        shell_quote_value(game_id)
    ))
}

fn shell_quote_path(path: &std::path::Path) -> String {
    let raw = path.to_string_lossy();
    if !raw.contains([' ', '\t', '"', '\'']) {
        return raw.into_owned();
    }
    format!("\"{}\"", raw.replace('\\', "\\\\").replace('"', "\\\""))
}

fn shell_quote_value(value: &str) -> String {
    if !value.contains([' ', '\t', '"', '\'']) {
        return value.to_string();
    }
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

impl Default for RuntimeSessionManager {
    fn default() -> Self {
        Self::new()
    }
}

pub fn global_runtime_manager() -> RuntimeSessionManager {
    static MANAGER: OnceLock<RuntimeSessionManager> = OnceLock::new();
    MANAGER.get_or_init(RuntimeSessionManager::new).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starting_and_stopping_session_updates_state() {
        let manager = RuntimeSessionManager::new();
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg("sleep 5");
        let (id, _) = manager
            .spawn_managed_session(
                SessionStart {
                    kind: SessionKind::Tool,
                    game_id: "g".to_string(),
                    tool_id: Some("t".to_string()),
                    launcher_source: GameLauncherSource::NonSteamUmu,
                    command: cmd,
                    completion: None,
                    vfs_mount: None,
                },
                None,
            )
            .unwrap();
        assert!(manager.any_active());
        manager.stop_session(id).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        let s = manager.sessions().into_iter().find(|s| s.id == id).unwrap();
        assert!(matches!(
            s.status,
            SessionStatus::Killed | SessionStatus::Exited
        ));
    }

    #[test]
    fn logs_are_captured() {
        let manager = RuntimeSessionManager::new();
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg("echo out; echo err 1>&2");
        let (id, _) = manager
            .spawn_managed_session(
                SessionStart {
                    kind: SessionKind::Game,
                    game_id: "g".to_string(),
                    tool_id: None,
                    launcher_source: GameLauncherSource::NonSteamUmu,
                    command: cmd,
                    completion: None,
                    vfs_mount: None,
                },
                None,
            )
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let logs = manager.session_logs(id);
        assert!(logs.iter().any(|l| l.message.contains("out")));
        assert!(logs.iter().any(|l| l.message.contains("err")));
    }

    #[cfg(unix)]
    #[test]
    fn session_waits_for_process_group_descendants() {
        let manager = RuntimeSessionManager::new();
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg("sleep 0.4 & exit 0");
        let (id, _) = manager
            .spawn_managed_session(
                SessionStart {
                    kind: SessionKind::Game,
                    game_id: "g".to_string(),
                    tool_id: None,
                    launcher_source: GameLauncherSource::NonSteamUmu,
                    command: cmd,
                    completion: None,
                    vfs_mount: None,
                },
                None,
            )
            .unwrap();

        std::thread::sleep(std::time::Duration::from_millis(100));
        let early = manager.sessions().into_iter().find(|s| s.id == id).unwrap();
        assert_eq!(early.status, SessionStatus::Running);

        std::thread::sleep(std::time::Duration::from_millis(600));
        let late = manager.sessions().into_iter().find(|s| s.id == id).unwrap();
        assert_eq!(late.status, SessionStatus::Exited);
    }

    #[test]
    fn steam_phase1_blocks_unlisted_games() {
        let manager = RuntimeSessionManager::new();
        let game = Game::new_steam(crate::core::games::GameKind::Fallout4, "/tmp/game".into());
        let err = manager.start_game_session(game).unwrap_err();
        assert!(err.contains("Phase 1 Steam session support"));
    }

    #[test]
    fn resolve_phase1_steam_game_prefers_current_game() {
        let game = Game::new_steam(crate::core::games::GameKind::SkyrimSE, "/tmp/skyrim".into());
        let game_id = game.id.clone();
        let config = AppConfig {
            current_game_id: Some(game_id),
            games: vec![game.clone()],
            ..AppConfig::default()
        };
        let resolved = resolve_phase1_steam_game(&config, 489830, None).unwrap();
        assert_eq!(resolved.id, game.id);
    }

    #[test]
    fn resolve_phase1_steam_exe_path_prefers_saved_target_when_present() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("SkyrimSE.exe"), b"").unwrap();
        std::fs::write(tmp.path().join("skse64_loader.exe"), b"").unwrap();
        let game = Game::new_steam(
            crate::core::games::GameKind::SkyrimSE,
            tmp.path().to_path_buf(),
        );
        let resolved = resolve_phase1_steam_exe_path(&game, Some("skse64_loader.exe")).unwrap();
        assert_eq!(resolved.file_name().unwrap(), "skse64_loader.exe");
    }

    #[test]
    fn resolve_phase1_steam_game_honors_requested_game_id() {
        let game_a = Game::new_steam(
            crate::core::games::GameKind::SkyrimSE,
            "/tmp/skyrim_a".into(),
        );
        let requested_id = game_a.id.clone();
        let game_b = Game::new_steam(
            crate::core::games::GameKind::SkyrimSE,
            "/tmp/skyrim_b".into(),
        );
        let config = AppConfig {
            games: vec![game_a.clone(), game_b],
            ..AppConfig::default()
        };
        let resolved = resolve_phase1_steam_game(&config, 489830, Some(&requested_id)).unwrap();
        assert_eq!(resolved.id, requested_id);
    }

    #[test]
    fn build_phase1_steam_launch_option_includes_game_id() {
        let launch_option = build_phase1_steam_launch_option(489830, "game-123").unwrap();
        assert!(launch_option.contains("--steam-session 489830"));
        assert!(launch_option.contains("--game-id game-123"));
    }
}
