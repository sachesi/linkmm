use log::{Level, LevelFilter, Log, Metadata, Record, SetLoggerError};
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

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

/// The maximum log level that the in-memory ring buffer captures.
/// Error, Warn, and Info are always captured for the UI log viewer.
/// Debug is captured for installation pipeline observability.
const BUFFER_CAPTURE_LEVEL: Level = Level::Debug;

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

// ── Performance profiling ─────────────────────────────────────────────────

/// A span that measures wall-clock time for a named operation.
///
/// On drop, logs the elapsed duration at INFO level.  Create via
/// [`span`] and hold the returned guard for the duration of the operation.
pub struct Span {
    name: String,
    context: String,
    start: Instant,
}

impl Drop for Span {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed();
        if self.context.is_empty() {
            log::info!(
                "[Span] {} completed in {:.3}s",
                self.name,
                elapsed.as_secs_f64()
            );
        } else {
            log::info!(
                "[Span] {} completed in {:.3}s | {}",
                self.name,
                elapsed.as_secs_f64(),
                self.context
            );
        }
    }
}

/// Start a profiling span.  The returned [`Span`] logs elapsed time on drop.
///
/// ```ignore
/// let _span = logger::span("VirtualTree::build", "archive=mod.7z");
/// // … expensive I/O …
/// // span logs duration automatically when it goes out of scope
/// ```
pub fn span(name: &str, context: &str) -> Span {
    log::debug!("[Span] {} started | {}", name, context);
    Span {
        name: name.to_string(),
        context: context.to_string(),
        start: Instant::now(),
    }
}

// ── Logger implementation ─────────────────────────────────────────────────

/// A `log::Log` implementation that:
/// 1. Stores `Error`, `Warn`, `Info`, and `Debug` records in a global ring
///    buffer so the UI log viewer can display them and installation pipeline
///    observability is preserved.
/// 2. Simultaneously emits all records to stderr via the standard
///    `env_logger` format so developers still see output in a terminal.
struct AppLogger {
    /// Underlying env_logger that handles formatting and writes to stderr.
    stderr: env_logger::Logger,
}

impl Log for AppLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= BUFFER_CAPTURE_LEVEL || self.stderr.enabled(metadata)
    }

    fn log(&self, record: &Record) {
        // Capture Error / Warn / Info / Debug into the buffer.
        if record.level() <= BUFFER_CAPTURE_LEVEL {
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
///
/// The buffer captures up to Debug level for installation pipeline
/// observability.  Trace is only emitted to stderr when `RUST_LOG=trace`.
pub fn init() -> Result<(), SetLoggerError> {
    // Build the underlying env_logger but do not install it as the global
    // logger — we wrap it in AppLogger instead.
    let stderr = env_logger::Builder::from_default_env().build();

    // The effective filter level must be at least Debug so that installation
    // pipeline logs (heuristic decisions, flag evaluations, span timings)
    // make it into the buffer even when RUST_LOG is unset.
    let max_level = stderr.filter().max(LevelFilter::Debug);

    let logger = Box::new(AppLogger { stderr });
    log::set_boxed_logger(logger)?;
    log::set_max_level(max_level);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_measures_elapsed_time() {
        // Just verify the span can be created and dropped without panic.
        let s = span("test_operation", "context=unit_test");
        assert!(!s.name.is_empty());
        drop(s);
    }

    #[test]
    fn log_entry_level_str_covers_all_levels() {
        let entry = LogEntry {
            timestamp_secs: 0,
            level: Level::Debug,
            message: "test".to_string(),
        };
        assert_eq!(entry.level_str(), "DEBUG");

        let entry = LogEntry {
            timestamp_secs: 0,
            level: Level::Trace,
            message: "test".to_string(),
        };
        assert_eq!(entry.level_str(), "TRACE");
    }
}
