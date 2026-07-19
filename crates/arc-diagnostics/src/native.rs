//! Native-only diagnostics adapters.
//!
//! This module is excluded from wasm builds and contains adapters that require
//! host filesystem access or native threads.

use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use anyhow::Context;
use rusqlite::{Connection, OptionalExtension, params};

/// Version metadata table used for sqlite diagnostics stores.
const META_TABLE_SQL: &str = "CREATE TABLE IF NOT EXISTS meta(version INTEGER)";

/// Open or create a versioned diagnostics sqlite database.
///
/// If an existing database has an incompatible version, the file is recreated.
pub fn open_or_create_versioned_sqlite(
    path: impl AsRef<Path>,
    schema_version: usize,
) -> anyhow::Result<Connection> {
    let path = path.as_ref();
    let mut con = Connection::open(path)?;
    con.execute_batch(META_TABLE_SQL)?;

    let current_version: Option<i64> =
        con.query_row("SELECT version FROM meta", [], |row| row.get(0)).optional()?;

    match current_version {
        None => {
            con.execute("INSERT INTO meta(version) VALUES (?1)", params![schema_version as i64])?;
        }
        Some(version) if version != schema_version as i64 => {
            match con.close() {
                Ok(()) => {
                    std::fs::remove_file(path).with_context(|| {
                        format!(
                            "failed to remove incompatible diagnostics sqlite at {}",
                            path.display()
                        )
                    })?;
                }
                Err((_, err)) => return Err(err.into()),
            }
            con = Connection::open(path)?;
            con.execute_batch(META_TABLE_SQL)?;
            con.execute("INSERT INTO meta(version) VALUES (?1)", params![schema_version as i64])?;
        }
        Some(_) => {}
    }

    con.execute_batch(
        "CREATE TABLE IF NOT EXISTS trace_event(
            id INTEGER PRIMARY KEY,
            level TEXT NOT NULL,
            message TEXT NOT NULL
        )",
    )?;

    Ok(con)
}

/// A trace event used by fan-out sinks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceEvent {
    /// Event severity level.
    pub level: String,
    /// Event message payload.
    pub message: String,
}

impl TraceEvent {
    /// Build a trace event.
    pub fn new(level: impl Into<String>, message: impl Into<String>) -> Self {
        Self { level: level.into(), message: message.into() }
    }
}

/// Consumer for trace events.
pub trait TraceSink: Send + Sync {
    /// Consume one trace event.
    fn write(&self, event: &TraceEvent) -> anyhow::Result<()>;
}

/// Fan-out trace dispatcher that writes each event to all registered sinks.
#[derive(Default)]
pub struct TraceFanout {
    sinks: Vec<Box<dyn TraceSink>>,
}

impl TraceFanout {
    /// Create an empty fan-out dispatcher.
    pub fn new() -> Self {
        Self { sinks: Vec::new() }
    }

    /// Register a new sink.
    pub fn add_sink(&mut self, sink: Box<dyn TraceSink>) {
        self.sinks.push(sink);
    }

    /// Dispatch one event to all sinks.
    pub fn emit(&self, event: &TraceEvent) -> anyhow::Result<()> {
        for sink in &self.sinks {
            sink.write(event)?;
        }
        Ok(())
    }
}

/// In-memory sink used for live progress rendering.
#[derive(Clone, Default)]
pub struct ProgressBufferSink {
    lines: Arc<Mutex<Vec<String>>>,
}

impl ProgressBufferSink {
    /// Build an empty progress sink.
    pub fn new() -> Self {
        Self { lines: Arc::new(Mutex::new(Vec::new())) }
    }

    /// Snapshot all recorded lines.
    pub fn lines(&self) -> Vec<String> {
        self.lines.lock().expect("progress buffer lock should not be poisoned").clone()
    }
}

impl TraceSink for ProgressBufferSink {
    fn write(&self, event: &TraceEvent) -> anyhow::Result<()> {
        self.lines
            .lock()
            .expect("progress buffer lock should not be poisoned")
            .push(format!("[{}] {}", event.level, event.message));
        Ok(())
    }
}

/// Sqlite-backed sink for persistent trace storage.
pub struct SqliteTraceSink {
    con: Arc<Mutex<Connection>>,
}

impl SqliteTraceSink {
    /// Build a sink from a shared sqlite connection.
    pub fn new(con: Arc<Mutex<Connection>>) -> Self {
        Self { con }
    }
}

impl TraceSink for SqliteTraceSink {
    fn write(&self, event: &TraceEvent) -> anyhow::Result<()> {
        self.con.lock().expect("sqlite sink lock should not be poisoned").execute(
            "INSERT INTO trace_event(level, message) VALUES (?1, ?2)",
            params![event.level, event.message],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[test]
    fn versioned_sqlite_bootstrap_creates_meta_and_trace_tables() {
        let temp_dir = tempfile::tempdir().expect("tempdir should be created");
        let db_path = temp_dir.path().join("diag.sqlite");

        let con = open_or_create_versioned_sqlite(&db_path, 3).expect("bootstrap should succeed");
        let version: i64 = con
            .query_row("SELECT version FROM meta", [], |row| row.get(0))
            .expect("version should exist");
        assert_eq!(version, 3_i64);

        let count: i64 = con
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='trace_event'",
                [],
                |row| row.get(0),
            )
            .expect("trace_event table should be queryable");
        assert_eq!(count, 1_i64);
    }

    #[test]
    fn fanout_writes_to_progress_and_sqlite_sinks() {
        let temp_dir = tempfile::tempdir().expect("tempdir should be created");
        let db_path = temp_dir.path().join("trace.sqlite");
        let con = open_or_create_versioned_sqlite(&db_path, 1).expect("bootstrap should succeed");
        let shared = Arc::new(Mutex::new(con));

        let progress = ProgressBufferSink::new();
        let sqlite_sink = SqliteTraceSink::new(shared.clone());

        let mut fanout = TraceFanout::new();
        fanout.add_sink(Box::new(progress.clone()));
        fanout.add_sink(Box::new(sqlite_sink));

        fanout
            .emit(&TraceEvent::new("info", "replay started"))
            .expect("fanout emit should succeed");

        let lines = progress.lines();
        assert_eq!(lines, vec!["[info] replay started".to_string()]);

        let persisted: i64 = shared
            .lock()
            .expect("sqlite lock should not be poisoned")
            .query_row("SELECT COUNT(*) FROM trace_event", [], |row| row.get(0))
            .expect("trace_event rows should be queryable");
        assert_eq!(persisted, 1_i64);
    }
}
