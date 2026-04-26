use crate::core::config::ToolConfig;
use crate::core::games::{Game, GameLauncherSource};
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
    Exited,
    Failed,
    Killed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionRunMode {
    FullyManaged,
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
    pub tool_id: Option<String>,
    pub launcher_source: GameLauncherSource,
    pub pid: Option<u32>,
    pub started_at: SystemTime,
    pub status: SessionStatus,
    pub exit_code: Option<i32>,
    pub run_mode: SessionRunMode,
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
            child
                .kill()
                .map_err(|e| format!("Failed to stop session {id}: {e}"))?;
        }
        Ok(())
    }

    pub fn start_game_session(
        &self,
        game: Game,
    ) -> Result<u64, String> {
        if game.launcher_source == GameLauncherSource::Steam {
            return Err(
                "Steam launches are launch-only and are not managed runtime sessions".to_string(),
            );
        }
        if self.current_game_session(&game.id).is_some() {
            return Err("A game session is already active for this game instance".to_string());
        }

        let umu = game.validate_umu_setup()?;
        let app_id = game.kind.primary_steam_app_id().unwrap_or(0);
        let command = crate::core::umu::build_umu_command(
            &umu.exe_path,
            "",
            app_id,
            umu.prefix_path.as_deref(),
            umu.proton_path.as_deref(),
        )?;

        self.spawn_managed_session(SessionStart {
            kind: SessionKind::Game,
            game_id: game.id,
            tool_id: None,
            launcher_source: game.launcher_source,
            command,
            completion: None,
        })
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
                )?
            }
            GameLauncherSource::Steam => {
                let app_id = game.steam_instance_app_id().unwrap_or(tool.app_id);
                crate::core::steam::build_tool_command(&tool.exe_path, &tool.arguments, app_id)?
            }
        };

        let id = self
            .spawn_managed_session(SessionStart {
                kind: SessionKind::Tool,
                game_id: game.id,
                tool_id: Some(tool.id),
                launcher_source: game.launcher_source,
                command,
                completion: Some(completion),
            })
            .map(|(id, _)| id)?;

        Ok((id, done_rx))
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
                        tool_id: start.tool_id,
                        launcher_source: start.launcher_source,
                        pid,
                        started_at: SystemTime::now(),
                        status: SessionStatus::Running,
                        exit_code: None,
                        run_mode: SessionRunMode::FullyManaged,
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
                tool_id: Some("t".to_string()),
                launcher_source: GameLauncherSource::NonSteamUmu,
                command: cmd,
                completion: None,
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
                tool_id: None,
                launcher_source: GameLauncherSource::NonSteamUmu,
                command: cmd,
                completion: None,
            })
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let logs = manager.session_logs(id);
        assert!(logs.iter().any(|l| l.message.contains("out")));
        assert!(logs.iter().any(|l| l.message.contains("err")));
    }

    #[test]
    fn steam_launches_are_not_managed_sessions() {
        let manager = RuntimeSessionManager::new();
        let game = Game::new_steam(crate::core::games::GameKind::SkyrimSE, "/tmp/game".into());
        let err = manager.start_game_session(game).unwrap_err();
        assert!(err.contains("launch-only"));
    }
}
