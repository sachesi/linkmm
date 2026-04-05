use crate::core::config::{ToolConfig, ToolRunProfile};
use crate::core::games::{Game, GameLauncherSource};
use crate::core::mods::{ModDatabase, ModManager};
use crate::core::tool_runs::{self, ToolRunResult};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader};
use std::process::{Child, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::SystemTime;

const MAX_LOG_LINES: usize = 600;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionKind {
    Game,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    Starting,
    Running,
    DelegatedRunning,
    Exited,
    Failed,
    Killed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone)]
pub struct SessionLogLine {
    pub stream: SessionStream,
    pub at: SystemTime,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct ManagedSession {
    pub id: u64,
    pub kind: SessionKind,
    pub game_id: String,
    pub profile_id: Option<String>,
    pub tool_id: Option<String>,
    pub launcher_source: GameLauncherSource,
    pub pid: Option<u32>,
    pub started_at: SystemTime,
    pub status: SessionStatus,
    pub exit_code: Option<i32>,
}

struct SessionControl {
    child: Mutex<Option<Child>>,
    stop_requested: AtomicBool,
}

struct SessionRecord {
    session: ManagedSession,
    logs: VecDeque<SessionLogLine>,
    control: Arc<SessionControl>,
    delegated_stop: Option<DelegatedStopKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DelegatedStopKind {
    SteamFlatpak,
    SteamNativeOrXdg,
    #[cfg(test)]
    NoopTest,
}

type SessionCompletion = Box<dyn FnOnce(ExitStatus) -> Result<(), String> + Send + 'static>;

struct SessionStart {
    kind: SessionKind,
    game_id: String,
    profile_id: Option<String>,
    tool_id: Option<String>,
    launcher_source: GameLauncherSource,
    command: std::process::Command,
    completion: Option<SessionCompletion>,
    delegated_stop: Option<DelegatedStopKind>,
}

#[derive(Clone)]
pub struct RuntimeSessionManager {
    inner: Arc<RuntimeSessionManagerInner>,
}

struct RuntimeSessionManagerInner {
    next_id: AtomicU64,
    sessions: Mutex<HashMap<u64, SessionRecord>>,
}

impl RuntimeSessionManager {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RuntimeSessionManagerInner {
                next_id: AtomicU64::new(1),
                sessions: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn any_active(&self) -> bool {
        self.sessions().into_iter().any(|s| {
            matches!(
                s.status,
                SessionStatus::Starting | SessionStatus::Running | SessionStatus::DelegatedRunning
            )
        })
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
                && matches!(
                    s.status,
                    SessionStatus::Starting
                        | SessionStatus::Running
                        | SessionStatus::DelegatedRunning
                )
        })
    }

    pub fn current_tool_session(&self, game_id: &str, tool_id: &str) -> Option<ManagedSession> {
        self.sessions().into_iter().find(|s| {
            s.kind == SessionKind::Tool
                && s.game_id == game_id
                && s.tool_id.as_deref() == Some(tool_id)
                && matches!(
                    s.status,
                    SessionStatus::Starting
                        | SessionStatus::Running
                        | SessionStatus::DelegatedRunning
                )
        })
    }

    pub fn stop_session(&self, id: u64) -> Result<(), String> {
        let (control, delegated_stop, status) = {
            let sessions = self.inner.sessions.lock().expect("sessions lock poisoned");
            let rec = sessions
                .get(&id)
                .ok_or_else(|| format!("Session {id} was not found"))?;
            (
                Arc::clone(&rec.control),
                rec.delegated_stop,
                rec.session.status.clone(),
            )
        };
        if matches!(status, SessionStatus::DelegatedRunning)
            && let Some(kind) = delegated_stop
        {
            self.stop_delegated_session(kind)?;
            let mut sessions = self.inner.sessions.lock().expect("sessions lock poisoned");
            if let Some(rec) = sessions.get_mut(&id) {
                rec.session.status = SessionStatus::Killed;
            }
            return Ok(());
        }
        control.stop_requested.store(true, Ordering::SeqCst);
        let mut child_guard = control.child.lock().expect("child lock poisoned");
        if let Some(child) = child_guard.as_mut() {
            child
                .kill()
                .map_err(|e| format!("Failed to stop session {id}: {e}"))?;
        }
        Ok(())
    }

    fn stop_delegated_session(&self, kind: DelegatedStopKind) -> Result<(), String> {
        let mut command = match kind {
            DelegatedStopKind::SteamFlatpak => {
                let mut cmd = std::process::Command::new("flatpak");
                cmd.arg("run")
                    .arg("com.valvesoftware.Steam")
                    .arg("-shutdown");
                cmd
            }
            DelegatedStopKind::SteamNativeOrXdg => {
                let mut cmd = std::process::Command::new("steam");
                cmd.arg("-shutdown");
                cmd
            }
            #[cfg(test)]
            DelegatedStopKind::NoopTest => return Ok(()),
        };
        command
            .spawn()
            .map_err(|e| format!("Failed issuing delegated Steam stop command: {e}"))?;
        Ok(())
    }

    pub fn start_game_session(
        &self,
        game: Game,
        profile_id: Option<String>,
    ) -> Result<u64, String> {
        if self.current_game_session(&game.id).is_some() {
            return Err("A game session is already active for this game instance".to_string());
        }

        let command = match game.launcher_source {
            GameLauncherSource::Steam => crate::core::steam::launch_game_managed_command(&game)?,
            GameLauncherSource::NonSteamUmu => {
                let umu = game.validate_umu_setup()?;
                let app_id = game.kind.steam_app_id().unwrap_or(0);
                crate::core::umu::build_umu_command(
                    &umu.exe_path,
                    app_id,
                    umu.prefix_path.as_deref(),
                    umu.proton_path.as_deref(),
                )?
            }
        };
        let delegated_stop = if game.launcher_source == GameLauncherSource::Steam {
            let program = command.get_program().to_string_lossy();
            if program == "flatpak" {
                Some(DelegatedStopKind::SteamFlatpak)
            } else if program == "xdg-open" {
                Some(DelegatedStopKind::SteamNativeOrXdg)
            } else {
                None
            }
        } else {
            None
        };

        self.spawn_managed_session(SessionStart {
            kind: SessionKind::Game,
            game_id: game.id,
            profile_id,
            tool_id: None,
            launcher_source: game.launcher_source,
            command,
            completion: None,
            delegated_stop,
        })
        .map(|(id, _)| id)
    }

    pub fn start_tool_session(
        &self,
        game: Game,
        profile_id: String,
        tool: ToolConfig,
        run_profile: ToolRunProfile,
    ) -> Result<(u64, mpsc::Receiver<Result<ToolRunResult, String>>), String> {
        if self.current_tool_session(&game.id, &tool.id).is_some() {
            return Err(format!("Tool {} is already running", tool.name));
        }

        let (done_tx, done_rx) = mpsc::channel::<Result<ToolRunResult, String>>();
        let game_clone = game.clone();
        let tool_clone = tool.clone();
        let run_profile_clone = run_profile.clone();

        let completion: SessionCompletion = Box::new(move |status: ExitStatus| {
            let mut db = ModDatabase::load(&game_clone);
            let result = tool_runs::run_tool_with_managed_outputs(
                &game_clone,
                &mut db,
                &tool_clone,
                &run_profile_clone,
                |_tool_cfg, _profile_cfg| Ok(status),
            );
            if result.is_ok() {
                let _ = ModManager::rebuild_all(&game_clone);
            }
            let _ = done_tx.send(result.clone());
            result.map(|_| ())
        });

        let command = self.tool_command(&game, &tool)?;

        let id = self
            .spawn_managed_session(SessionStart {
                kind: SessionKind::Tool,
                game_id: game.id,
                profile_id: Some(profile_id),
                tool_id: Some(tool.id),
                launcher_source: game.launcher_source,
                command,
                completion: Some(completion),
                delegated_stop: None,
            })
            .map(|(id, _)| id)?;

        Ok((id, done_rx))
    }

    fn tool_command(
        &self,
        game: &Game,
        tool: &ToolConfig,
    ) -> Result<std::process::Command, String> {
        match game.launcher_source {
            GameLauncherSource::Steam => {
                crate::core::steam::build_tool_command(&tool.exe_path, &tool.arguments, tool.app_id)
            }
            GameLauncherSource::NonSteamUmu => {
                let umu = game.validate_umu_setup()?;
                crate::core::umu::build_umu_command(
                    &tool.exe_path,
                    tool.app_id,
                    umu.prefix_path.as_deref(),
                    umu.proton_path.as_deref(),
                )
            }
        }
    }

    fn spawn_managed_session(
        &self,
        mut start: SessionStart,
    ) -> Result<(u64, Arc<SessionControl>), String> {
        start.command.stdout(Stdio::piped());
        start.command.stderr(Stdio::piped());

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
                        profile_id: start.profile_id,
                        tool_id: start.tool_id,
                        launcher_source: start.launcher_source,
                        pid,
                        started_at: SystemTime::now(),
                        status: SessionStatus::Running,
                        exit_code: None,
                    },
                    logs: VecDeque::new(),
                    control: Arc::clone(&control),
                    delegated_stop: start.delegated_stop,
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
        std::thread::spawn(move || {
            let status_result = {
                let mut child_guard = control_for_wait.child.lock().expect("child lock poisoned");
                child_guard
                    .take()
                    .ok_or_else(|| "Session process handle missing".to_string())
                    .and_then(|mut child| {
                        child
                            .wait()
                            .map_err(|e| format!("Failed waiting for managed session: {e}"))
                    })
            };

            manager.finish_session(
                id,
                status_result,
                start.completion,
                control_for_wait.stop_requested.load(Ordering::SeqCst),
            );
        });

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
                } else if self
                    .inner
                    .sessions
                    .lock()
                    .expect("sessions lock poisoned")
                    .get(&id)
                    .and_then(|r| r.delegated_stop)
                    .is_some()
                    && status.success()
                {
                    SessionStatus::DelegatedRunning
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
            .spawn_managed_session(SessionStart {
                kind: SessionKind::Tool,
                game_id: "g".to_string(),
                profile_id: Some("p".to_string()),
                tool_id: Some("t".to_string()),
                launcher_source: GameLauncherSource::Steam,
                command: cmd,
                completion: None,
                delegated_stop: None,
            })
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
            .spawn_managed_session(SessionStart {
                kind: SessionKind::Game,
                game_id: "g".to_string(),
                profile_id: None,
                tool_id: None,
                launcher_source: GameLauncherSource::Steam,
                command: cmd,
                completion: None,
                delegated_stop: None,
            })
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let logs = manager.session_logs(id);
        assert!(logs.iter().any(|l| l.message.contains("out")));
        assert!(logs.iter().any(|l| l.message.contains("err")));
    }

    #[test]
    fn delegated_session_stays_active_after_wrapper_exit() {
        let manager = RuntimeSessionManager::new();
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg("echo delegated");
        let (id, _) = manager
            .spawn_managed_session(SessionStart {
                kind: SessionKind::Game,
                game_id: "g".to_string(),
                profile_id: None,
                tool_id: None,
                launcher_source: GameLauncherSource::Steam,
                command: cmd,
                completion: None,
                delegated_stop: Some(DelegatedStopKind::NoopTest),
            })
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(250));
        let s = manager.sessions().into_iter().find(|s| s.id == id).unwrap();
        assert_eq!(s.status, SessionStatus::DelegatedRunning);
        assert!(manager.current_game_session("g").is_some());
    }

    #[test]
    fn stopping_delegated_session_marks_killed() {
        let manager = RuntimeSessionManager::new();
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg("true");
        let (id, _) = manager
            .spawn_managed_session(SessionStart {
                kind: SessionKind::Game,
                game_id: "g".to_string(),
                profile_id: None,
                tool_id: None,
                launcher_source: GameLauncherSource::Steam,
                command: cmd,
                completion: None,
                delegated_stop: Some(DelegatedStopKind::NoopTest),
            })
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        manager.stop_session(id).unwrap();
        let s = manager.sessions().into_iter().find(|s| s.id == id).unwrap();
        assert_eq!(s.status, SessionStatus::Killed);
    }
}
