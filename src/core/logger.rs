use log::{Level, LevelFilter, Log, Metadata, Record, SetLoggerError};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

// ── Log entry ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct LogEntry {
    /// Seconds since UNIX epoch (UTC).
    pub timestamp_secs: u64,
    pub level: Level,
    pub message: String,
}

impl LogEntry {
    /// Format the timestamp as `HH:MM:SS UTC`.
    pub fn time_str(&self) -> String {
        let secs = self.timestamp_secs;
        // The timestamp is seconds since UNIX epoch (UTC).
        let h = (secs / 3600) % 24;
        let m = (secs / 60) % 60;
        let s = secs % 60;
        format!("{h:02}:{m:02}:{s:02} UTC")
    }

    /// Short uppercase label for the log level.
    pub fn level_str(&self) -> &'static str {
        match self.level {
            Level::Error => "ERROR",
            Level::Warn => "WARN ",
            Level::Info => "INFO ",
            Level::Debug => "DEBUG",
            Level::Trace => "TRACE",
        }
    }
}

// ── Global buffer ──────────────────────────────────────────────────────────

/// Maximum number of entries kept in the in-memory ring buffer.
const MAX_ENTRIES: usize = 10_000;

static LOG_BUFFER: OnceLock<Mutex<Vec<LogEntry>>> = OnceLock::new();

fn log_buffer() -> &'static Mutex<Vec<LogEntry>> {
    LOG_BUFFER.get_or_init(|| Mutex::new(Vec::new()))
}

/// Return a snapshot of all captured log entries.
pub fn get_logs() -> Vec<LogEntry> {
    match log_buffer().lock() {
        Ok(buf) => buf.clone(),
        Err(p) => p.into_inner().clone(),
    }
}

// ── Logger implementation ─────────────────────────────────────────────────

/// A `log::Log` implementation that:
/// 1. Stores `Error`, `Warn`, and `Info` records in a global ring buffer so
///    the UI log viewer can display them.
/// 2. Simultaneously emits all records to stderr via the standard
///    `env_logger` format so developers still see output in a terminal.
struct AppLogger {
    /// Underlying env_logger that handles formatting and writes to stderr.
    stderr: env_logger::Logger,
}

impl Log for AppLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        // Accept up to Info level ourselves; anything finer is still forwarded
        // to the stderr logger which may filter it further via RUST_LOG.
        metadata.level() <= Level::Info || self.stderr.enabled(metadata)
    }

    fn log(&self, record: &Record) {
        // Capture Error / Warn / Info into the buffer.
        if record.level() <= Level::Info {
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let entry = LogEntry {
                timestamp_secs: ts,
                level: record.level(),
                message: record.args().to_string(),
            };
            if let Ok(mut buf) = log_buffer().lock() {
                buf.push(entry);
                if buf.len() > MAX_ENTRIES {
                    let excess = buf.len() - MAX_ENTRIES;
                    buf.drain(0..excess);
                }
            }
        }

        // Always forward to the stderr logger so `RUST_LOG` still works.
        if self.stderr.enabled(record.metadata()) {
            self.stderr.log(record);
        }
    }

    fn flush(&self) {
        self.stderr.flush();
    }
}

// ── Public initialiser ────────────────────────────────────────────────────

/// Initialise the global logger.
///
/// Replaces the default `env_logger::init()` call.  All `log::*!()` macros
/// will write to both stderr (respecting `RUST_LOG`) and the in-memory buffer
/// that the UI log viewer reads from.
pub fn init() -> Result<(), SetLoggerError> {
    // Build the underlying env_logger but do not install it as the global
    // logger — we wrap it in AppLogger instead.
    let stderr = env_logger::Builder::from_default_env().build();

    // The effective filter level must be at least Info so that activity/
    // installation log lines make it into the buffer even when RUST_LOG is
    // unset (which defaults to Error-only).
    let max_level = stderr.filter().max(LevelFilter::Info);

    let logger = Box::new(AppLogger { stderr });
    log::set_boxed_logger(logger)?;
    log::set_max_level(max_level);
    Ok(())
}
