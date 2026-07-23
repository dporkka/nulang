//! Persistence engine for durable actors.
//!
//! v0.7 MVP: in-memory store plus JSON file backend. The store keeps a
//! snapshot of durable actor state and an append-only journal of messages.
//! On recovery the runtime loads the latest snapshot and replays the journal.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::vm::Value;

use tracing::warn;

/// How a state field is persisted / replicated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum StateModel {
    /// Ephemeral, reset on restart.
    Local,
    /// Snapshot + journal, survives restart.
    Durable,
    /// Event journal with deterministic replay.
    EventSourced,
    /// CRDT, merged across the cluster.
    Crdt,
}

impl StateModel {
    pub fn is_persistent(self) -> bool {
        matches!(
            self,
            StateModel::Durable | StateModel::EventSourced | StateModel::Crdt
        )
    }
}

/// A serializable stand-in for `Value`. Pointers and strings are not safely
/// restorable outside the VM, so they are normalized to nil.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "tag", content = "value")]
pub enum PersistedValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Nil,
    Unit,
    Actor(u64),
}

impl PersistedValue {
    pub fn from_value(v: &Value) -> Self {
        if let Some(i) = v.as_int() {
            PersistedValue::Int(i)
        } else if let Some(f) = v.as_float() {
            PersistedValue::Float(f)
        } else if let Some(b) = v.as_bool() {
            PersistedValue::Bool(b)
        } else if v.is_nil() {
            PersistedValue::Nil
        } else if v.is_unit() {
            PersistedValue::Unit
        } else if let Some(a) = v.as_actor_id() {
            PersistedValue::Actor(a)
        } else {
            // Pointers and string references cannot be safely restored without
            // the owning heap / constant pool, so they normalize to nil.
            PersistedValue::Nil
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            PersistedValue::Int(i) => Value::int(*i),
            PersistedValue::Float(f) => Value::float(*f),
            PersistedValue::Bool(b) => Value::bool(*b),
            PersistedValue::String(_) => Value::nil(),
            PersistedValue::Nil => Value::nil(),
            PersistedValue::Unit => Value::unit(),
            PersistedValue::Actor(a) => Value::actor_ref(*a),
        }
    }
}

/// A serializable snapshot of an actor's durable state.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ActorSnapshot {
    pub actor_id: u64,
    pub sequence: u64,
    pub state: HashMap<String, PersistedValue>,
    /// For workflow actors, the name of the signal the current step is
    /// suspended waiting for, if any.  This is part of the snapshot so that
    /// recovery can decide whether the in-flight step must be re-triggered.
    pub waiting_signal: Option<String>,
}

/// A journal entry records a message delivered to an actor.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct JournalEntry {
    pub sequence: u64,
    pub behavior_id: u16,
    pub payload: Vec<PersistedValue>,
}

/// An event-sourced state change. Appended to the event log for each
/// mutation of an EventSourced field. On recovery, events are replayed
/// to reconstruct the field's current value without a full snapshot.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EventEntry {
    pub sequence: u64,
    /// Name of the EventSourced field being mutated.
    pub field_name: String,
    /// Event name (e.g. "Incremented", "Custom").
    pub event_name: String,
    /// Event arguments.
    pub args: Vec<PersistedValue>,
}

/// A workflow event records a durable, replayable step in a workflow actor.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "tag", content = "value")]
pub enum WorkflowEvent {
    /// Workflow instance started. `state` captures durable fields at creation.
    WorkflowStarted {
        sequence: u64,
        name: String,
        state: Vec<PersistedValue>,
    },
    /// A workflow step completed successfully.
    StepCompleted { sequence: u64, step_name: String },
    /// A timer was set for a workflow.
    TimerSet {
        sequence: u64,
        name: String,
        duration_ms: u64,
    },
    /// A previously set timer fired.
    TimerFired { sequence: u64, name: String },
    /// An external signal was delivered to the workflow.
    SignalReceived {
        sequence: u64,
        name: String,
        payload: Option<String>,
    },
    /// A saga step was compensated after failure.
    SagaCompensated { sequence: u64, step_name: String },
    /// A branch of a synthetic parallel step completed.
    ParallelBranchCompleted {
        sequence: u64,
        parallel_step_name: String,
        branch_name: String,
    },
    /// Any other event emitted by a workflow handler.
    Custom {
        sequence: u64,
        name: String,
        args: Vec<PersistedValue>,
    },
}

impl WorkflowEvent {
    /// Return the sequence number of this event.
    pub fn sequence(&self) -> u64 {
        match self {
            WorkflowEvent::WorkflowStarted { sequence, .. }
            | WorkflowEvent::StepCompleted { sequence, .. }
            | WorkflowEvent::TimerSet { sequence, .. }
            | WorkflowEvent::TimerFired { sequence, .. }
            | WorkflowEvent::SignalReceived { sequence, .. }
            | WorkflowEvent::SagaCompensated { sequence, .. }
            | WorkflowEvent::ParallelBranchCompleted { sequence, .. }
            | WorkflowEvent::Custom { sequence, .. } => *sequence,
        }
    }
}

/// Persistence backend trait. Implementations may be in-memory or disk-backed.
pub trait PersistenceStore: Send + Sync {
    /// Persist a snapshot of durable actor state.
    fn save_snapshot(&mut self, snapshot: ActorSnapshot) -> io::Result<()>;

    /// Load the latest snapshot for an actor, if any.
    fn load_snapshot(&self, actor_id: u64) -> Option<ActorSnapshot>;

    /// Append a message to the actor's journal.
    fn append_journal(&mut self, actor_id: u64, entry: JournalEntry) -> io::Result<()>;

    /// Read all journal entries for an actor in order.
    fn read_journal(&self, actor_id: u64) -> Vec<JournalEntry>;

    /// Append a workflow event to the actor's event journal.
    fn append_workflow_event(&mut self, actor_id: u64, event: WorkflowEvent) -> io::Result<()>;

    /// Read all workflow events for an actor in order.
    fn read_workflow_events(&self, actor_id: u64) -> Vec<WorkflowEvent>;

    /// Append a `TimerSet` workflow event.
    fn append_timer_set(
        &mut self,
        actor_id: u64,
        sequence: u64,
        name: String,
        duration_ms: u64,
    ) -> io::Result<()> {
        self.append_workflow_event(
            actor_id,
            WorkflowEvent::TimerSet {
                sequence,
                name,
                duration_ms,
            },
        )
    }

    /// Append a `TimerFired` workflow event.
    fn append_timer_fired(&mut self, actor_id: u64, sequence: u64, name: String) -> io::Result<()> {
        self.append_workflow_event(actor_id, WorkflowEvent::TimerFired { sequence, name })
    }

    /// Append a `SignalReceived` workflow event.
    fn append_signal_received(
        &mut self,
        actor_id: u64,
        sequence: u64,
        name: String,
        payload: Option<String>,
    ) -> io::Result<()> {
        self.append_workflow_event(
            actor_id,
            WorkflowEvent::SignalReceived {
                sequence,
                name,
                payload,
            },
        )
    }

    /// Append a `SagaCompensated` workflow event.
    fn append_saga_compensated(
        &mut self,
        actor_id: u64,
        sequence: u64,
        step_name: String,
    ) -> io::Result<()> {
        self.append_workflow_event(
            actor_id,
            WorkflowEvent::SagaCompensated {
                sequence,
                step_name,
            },
        )
    }

    /// Read timer-related workflow events (`TimerSet` and `TimerFired`).
    fn read_timer_events(&self, actor_id: u64) -> Vec<WorkflowEvent> {
        self.read_workflow_events(actor_id)
            .into_iter()
            .filter(|e| {
                matches!(
                    e,
                    WorkflowEvent::TimerSet { .. } | WorkflowEvent::TimerFired { .. }
                )
            })
            .collect()
    }

    /// Read `SignalReceived` workflow events.
    fn read_signal_events(&self, actor_id: u64) -> Vec<WorkflowEvent> {
        self.read_workflow_events(actor_id)
            .into_iter()
            .filter(|e| matches!(e, WorkflowEvent::SignalReceived { .. }))
            .collect()
    }

    /// Read `SagaCompensated` workflow events.
    fn read_saga_events(&self, actor_id: u64) -> Vec<WorkflowEvent> {
        self.read_workflow_events(actor_id)
            .into_iter()
            .filter(|e| matches!(e, WorkflowEvent::SagaCompensated { .. }))
            .collect()
    }

    /// Append a `ParallelBranchCompleted` workflow event.
    fn append_parallel_branch_completed(
        &mut self,
        actor_id: u64,
        sequence: u64,
        parallel_step_name: String,
        branch_name: String,
    ) -> io::Result<()> {
        self.append_workflow_event(
            actor_id,
            WorkflowEvent::ParallelBranchCompleted {
                sequence,
                parallel_step_name,
                branch_name,
            },
        )
    }

    /// Read `ParallelBranchCompleted` workflow events.
    fn read_parallel_branch_events(&self, actor_id: u64) -> Vec<WorkflowEvent> {
        self.read_workflow_events(actor_id)
            .into_iter()
            .filter(|e| matches!(e, WorkflowEvent::ParallelBranchCompleted { .. }))
            .collect()
    }

    /// Append an event to the actor's event-sourcing log.
    fn append_event(&mut self, actor_id: u64, entry: EventEntry) -> io::Result<()>;

    /// Read all event-sourcing entries for an actor in order.
    fn read_events(&self, actor_id: u64) -> Vec<EventEntry>;

    /// Highest sequence number known for the actor.
    fn latest_sequence(&self, actor_id: u64) -> u64;

    /// Remove all data for an actor.
    fn clear(&mut self, actor_id: u64) -> io::Result<()>;

    /// Execute an arbitrary SQL query against the store.
    /// Returns rows as JSON arrays of column values. Default: not supported.
    fn query(&self, _sql: &str, _params: &[Value]) -> io::Result<Vec<String>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "DB.query is not supported by this persistence backend",
        ))
    }
}

/// In-memory persistence store. Useful for tests and ephemeral durable actors.
#[derive(Debug, Default, Clone)]
pub struct MemoryStore {
    snapshots: HashMap<u64, ActorSnapshot>,
    journals: HashMap<u64, Vec<JournalEntry>>,
    workflow_events: HashMap<u64, Vec<WorkflowEvent>>,
    events: HashMap<u64, Vec<EventEntry>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl PersistenceStore for MemoryStore {
    fn save_snapshot(&mut self, snapshot: ActorSnapshot) -> io::Result<()> {
        self.snapshots.insert(snapshot.actor_id, snapshot);
        Ok(())
    }

    fn load_snapshot(&self, actor_id: u64) -> Option<ActorSnapshot> {
        self.snapshots.get(&actor_id).cloned()
    }

    fn append_journal(&mut self, actor_id: u64, entry: JournalEntry) -> io::Result<()> {
        self.journals.entry(actor_id).or_default().push(entry);
        Ok(())
    }

    fn read_journal(&self, actor_id: u64) -> Vec<JournalEntry> {
        self.journals.get(&actor_id).cloned().unwrap_or_default()
    }

    fn append_workflow_event(&mut self, actor_id: u64, event: WorkflowEvent) -> io::Result<()> {
        self.workflow_events
            .entry(actor_id)
            .or_default()
            .push(event);
        Ok(())
    }

    fn read_workflow_events(&self, actor_id: u64) -> Vec<WorkflowEvent> {
        self.workflow_events
            .get(&actor_id)
            .cloned()
            .unwrap_or_default()
    }

    fn append_event(&mut self, actor_id: u64, entry: EventEntry) -> io::Result<()> {
        self.events.entry(actor_id).or_default().push(entry);
        Ok(())
    }

    fn read_events(&self, actor_id: u64) -> Vec<EventEntry> {
        self.events.get(&actor_id).cloned().unwrap_or_default()
    }

    fn latest_sequence(&self, actor_id: u64) -> u64 {
        let snapshot_seq = self
            .snapshots
            .get(&actor_id)
            .map(|s| s.sequence)
            .unwrap_or(0);
        let journal_seq = self
            .journals
            .get(&actor_id)
            .and_then(|j| j.last().map(|e| e.sequence))
            .unwrap_or(0);
        let wf_event_seq = self
            .workflow_events
            .get(&actor_id)
            .and_then(|e| e.last().map(|ev| ev.sequence()))
            .unwrap_or(0);
        let event_seq = self
            .events
            .get(&actor_id)
            .and_then(|e| e.last().map(|ev| ev.sequence))
            .unwrap_or(0);
        snapshot_seq
            .max(journal_seq)
            .max(wf_event_seq)
            .max(event_seq)
    }

    fn clear(&mut self, actor_id: u64) -> io::Result<()> {
        self.snapshots.remove(&actor_id);
        self.journals.remove(&actor_id);
        self.workflow_events.remove(&actor_id);
        self.events.remove(&actor_id);
        Ok(())
    }
}

/// File-backed persistence store using JSON.
/// Each actor gets `<base_dir>/<actor_id>/snapshot.json`, `journal.jsonl`,
/// and `workflow_events.jsonl`.
#[derive(Debug, Clone)]
pub struct JsonFileStore {
    base_dir: PathBuf,
}

impl JsonFileStore {
    pub fn new<P: AsRef<Path>>(base_dir: P) -> io::Result<Self> {
        let base_dir = base_dir.as_ref().to_path_buf();
        fs::create_dir_all(&base_dir)?;
        Ok(JsonFileStore { base_dir })
    }

    fn actor_dir(&self, actor_id: u64) -> PathBuf {
        self.base_dir.join(format!("actor_{}", actor_id))
    }

    fn snapshot_path(&self, actor_id: u64) -> PathBuf {
        self.actor_dir(actor_id).join("snapshot.json")
    }

    fn journal_path(&self, actor_id: u64) -> PathBuf {
        self.actor_dir(actor_id).join("journal.jsonl")
    }

    fn workflow_events_path(&self, actor_id: u64) -> PathBuf {
        self.actor_dir(actor_id).join("workflow_events.jsonl")
    }

    fn events_path(&self, actor_id: u64) -> PathBuf {
        self.actor_dir(actor_id).join("events.jsonl")
    }
}

impl PersistenceStore for JsonFileStore {
    fn save_snapshot(&mut self, snapshot: ActorSnapshot) -> io::Result<()> {
        let dir = self.actor_dir(snapshot.actor_id);
        fs::create_dir_all(&dir)?;
        let path = self.snapshot_path(snapshot.actor_id);
        let json = serde_json::to_string_pretty(&snapshot)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        // Write to a temp file in the same directory, then atomically rename
        // it into place: a crash mid-write can no longer leave a truncated
        // snapshot.json that recovery would silently treat as "no state".
        let tmp_path = dir.join("snapshot.json.tmp");
        {
            let mut file = fs::File::create(&tmp_path)?;
            file.write_all(json.as_bytes())?;
            file.sync_all()?;
        }
        fs::rename(&tmp_path, &path)?;
        Ok(())
    }

    fn load_snapshot(&self, actor_id: u64) -> Option<ActorSnapshot> {
        let path = self.snapshot_path(actor_id);
        // A missing file is the normal "no snapshot yet" case — stay silent.
        let data = fs::read_to_string(&path).ok()?;
        match serde_json::from_str(&data) {
            Ok(snapshot) => Some(snapshot),
            Err(e) => {
                // A present-but-unparseable snapshot means corruption (e.g. an
                // older non-atomic write); log it instead of silently resetting
                // the actor's durable state on recovery.
                warn!(
                    "nulang-persist: failed to parse snapshot for actor {} at {}: {}",
                    actor_id,
                    path.display(),
                    e
                );
                None
            }
        }
    }

    fn append_journal(&mut self, actor_id: u64, entry: JournalEntry) -> io::Result<()> {
        let dir = self.actor_dir(actor_id);
        fs::create_dir_all(&dir)?;
        let path = self.journal_path(actor_id);
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        let json = serde_json::to_string(&entry)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        writeln!(file, "{}", json)?;
        Ok(())
    }

    fn read_journal(&self, actor_id: u64) -> Vec<JournalEntry> {
        let path = self.journal_path(actor_id);
        let data = match fs::read_to_string(path) {
            Ok(d) => d,
            Err(_) => return Vec::new(),
        };
        data.lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    fn append_workflow_event(&mut self, actor_id: u64, event: WorkflowEvent) -> io::Result<()> {
        let dir = self.actor_dir(actor_id);
        fs::create_dir_all(&dir)?;
        let path = self.workflow_events_path(actor_id);
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        let json = serde_json::to_string(&event)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        writeln!(file, "{}", json)?;
        Ok(())
    }

    fn read_workflow_events(&self, actor_id: u64) -> Vec<WorkflowEvent> {
        let path = self.workflow_events_path(actor_id);
        let data = match fs::read_to_string(path) {
            Ok(d) => d,
            Err(_) => return Vec::new(),
        };
        data.lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    fn append_event(&mut self, actor_id: u64, entry: EventEntry) -> io::Result<()> {
        let dir = self.actor_dir(actor_id);
        fs::create_dir_all(&dir)?;
        let path = self.events_path(actor_id);
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        let json = serde_json::to_string(&entry)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        writeln!(file, "{}", json)?;
        Ok(())
    }

    fn read_events(&self, actor_id: u64) -> Vec<EventEntry> {
        let path = self.events_path(actor_id);
        let data = match fs::read_to_string(path) {
            Ok(d) => d,
            Err(_) => return Vec::new(),
        };
        data.lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    fn latest_sequence(&self, actor_id: u64) -> u64 {
        let snapshot_seq = self
            .load_snapshot(actor_id)
            .map(|s| s.sequence)
            .unwrap_or(0);
        let journal_seq = self
            .read_journal(actor_id)
            .last()
            .map(|e| e.sequence)
            .unwrap_or(0);
        let wf_event_seq = self
            .read_workflow_events(actor_id)
            .last()
            .map(|e| e.sequence())
            .unwrap_or(0);
        let event_seq = self
            .read_events(actor_id)
            .last()
            .map(|e| e.sequence)
            .unwrap_or(0);
        snapshot_seq
            .max(journal_seq)
            .max(wf_event_seq)
            .max(event_seq)
    }

    fn clear(&mut self, actor_id: u64) -> io::Result<()> {
        let dir = self.actor_dir(actor_id);
        if dir.exists() {
            fs::remove_dir_all(dir)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// LibsqlStore — libSQL-backed persistence (local, remote Turso, or replica)
// ---------------------------------------------------------------------------

/// libSQL-backed persistence store.
///
/// Each actor gets one row in the `snapshots` table and zero or more rows in
/// the `journal` and `workflow_events` tables, same schema as the old SQLite
/// store.  State and payloads are serialized to JSON and stored as TEXT.
///
/// The same store also serves `perform DB.query(sql, params)` from Nulang
/// code via the `query()` method exposed through `PersistenceStore::query`.
#[cfg(feature = "sqlite")]
pub struct LibsqlStore {
    conn: std::sync::Mutex<libsql::Connection>,
    rt: tokio::runtime::Runtime,
    path: PathBuf,
}

#[cfg(feature = "sqlite")]
impl LibsqlStore {
    /// Open (or create) a local file database.  Pass `":memory:"` for an
    /// ephemeral in-memory store.
    pub fn new<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let db_path = if path == Path::new(":memory:") {
            ":memory:".to_string()
        } else {
            path.to_string_lossy().into_owned()
        };
        let rt =
            tokio::runtime::Runtime::new().map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        let db = rt.block_on(async {
            libsql::Builder::new_local(&db_path)
                .build()
                .await
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))
        })?;
        let conn = db
            .connect()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
        let store = LibsqlStore {
            conn: std::sync::Mutex::new(conn),
            rt,
            path,
        };
        store.ensure_tables()?;
        Ok(store)
    }
    pub fn in_memory() -> io::Result<Self> {
        Self::new(":memory:")
    }

    /// Connect to a remote Turso database.
    pub fn new_remote(url: &str, auth_token: &str) -> io::Result<Self> {
        let rt =
            tokio::runtime::Runtime::new().map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        let db = rt.block_on(async {
            libsql::Builder::new_remote(url.to_string(), auth_token.to_string())
                .build()
                .await
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))
        })?;
        let conn = db
            .connect()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
        let store = LibsqlStore {
            conn: std::sync::Mutex::new(conn),
            rt,
            path: PathBuf::from(url),
        };
        store.ensure_tables()?;
        Ok(store)
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// Acquire the database connection lock.
    fn conn(&self) -> std::sync::MutexGuard<'_, libsql::Connection> {
        self.conn.lock().unwrap()
    }

    fn ensure_tables(&self) -> io::Result<()> {
        let conn = self.conn();
        self.rt.block_on(async {
            conn.execute(
                "CREATE TABLE IF NOT EXISTS snapshots (
                    actor_id INTEGER PRIMARY KEY,
                    sequence INTEGER NOT NULL,
                    state TEXT NOT NULL,
                    waiting_signal TEXT
                )",
                (),
            )
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
            // Migrate databases created before the waiting_signal column existed.
            let _ = conn
                .execute("ALTER TABLE snapshots ADD COLUMN waiting_signal TEXT", ())
                .await;
            conn.execute(
                "CREATE TABLE IF NOT EXISTS journal (
                    actor_id INTEGER NOT NULL,
                    sequence INTEGER NOT NULL,
                    behavior_id INTEGER NOT NULL,
                    payload TEXT NOT NULL,
                    PRIMARY KEY (actor_id, sequence)
                )",
                (),
            )
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
            conn.execute(
                "CREATE TABLE IF NOT EXISTS workflow_events (
                    actor_id INTEGER NOT NULL,
                    sequence INTEGER NOT NULL,
                    event TEXT NOT NULL,
                    PRIMARY KEY (actor_id, sequence)
                )",
                (),
            )
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
            conn.execute(
                "CREATE TABLE IF NOT EXISTS events (
                    actor_id INTEGER NOT NULL,
                    sequence INTEGER NOT NULL,
                    field_name TEXT NOT NULL,
                    event_name TEXT NOT NULL,
                    args TEXT NOT NULL,
                    PRIMARY KEY (actor_id, sequence)
                )",
                (),
            )
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
            Ok(())
        })
    }

    /// Execute a SQL query and return rows as a Vec of JSON strings.
    pub fn query(&self, sql: &str, params: &[Value]) -> io::Result<Vec<String>> {
        let conn = self.conn();
        self.rt.block_on(async {
            let param_values: Vec<String> = params
                .iter()
                .map(|v| {
                    if let Some(i) = v.as_int() {
                        i.to_string()
                    } else if let Some(f) = v.as_float() {
                        f.to_string()
                    } else if let Some(b) = v.as_bool() {
                        b.to_string()
                    } else {
                        v.to_string_repr()
                    }
                })
                .collect();
            let param_refs: Vec<&str> = param_values.iter().map(|s| s.as_str()).collect();
            let mut rows = conn
                .query(sql, libsql::params_from_iter(param_refs))
                .await
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
            let mut results = Vec::new();
            loop {
                match rows.next().await {
                    Ok(Some(row)) => {
                        let mut cols: Vec<serde_json::Value> = Vec::new();
                        for i in 0..row.column_count() {
                            let val = row
                                .get_value(i)
                                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                            let json_val = match val {
                                libsql::Value::Null => serde_json::Value::Null,
                                libsql::Value::Integer(n) => {
                                    serde_json::value::Number::from_i128(n as i128)
                                        .map(serde_json::Value::Number)
                                        .unwrap_or(serde_json::Value::Null)
                                }
                                libsql::Value::Real(f) => serde_json::value::Number::from_f64(f)
                                    .map(serde_json::Value::Number)
                                    .unwrap_or(serde_json::Value::Null),
                                libsql::Value::Text(s) => serde_json::Value::String(s),
                                libsql::Value::Blob(_) => serde_json::Value::Null,
                            };
                            cols.push(json_val);
                        }
                        let json = serde_json::to_string(&cols)
                            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
                        results.push(json);
                    }
                    Ok(None) => break,
                    Err(e) => return Err(io::Error::new(io::ErrorKind::Other, e.to_string())),
                }
            }
            Ok(results)
        })
    }
}

#[cfg(feature = "sqlite")]
impl PersistenceStore for LibsqlStore {
    fn save_snapshot(&mut self, snapshot: ActorSnapshot) -> io::Result<()> {
        let state_json = serde_json::to_string(&snapshot.state)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let conn = self.conn();
        self.rt.block_on(async {
            conn.execute(
                "INSERT INTO snapshots (actor_id, sequence, state, waiting_signal) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(actor_id) DO UPDATE SET sequence=excluded.sequence, state=excluded.state, waiting_signal=excluded.waiting_signal",
                libsql::params![snapshot.actor_id as i64, snapshot.sequence as i64, state_json, snapshot.waiting_signal.as_deref()],
            ).await.map(|_| ()).map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))
        })
    }

    fn load_snapshot(&self, actor_id: u64) -> Option<ActorSnapshot> {
        let conn = self.conn();
        self.rt.block_on(async {
            let mut rows = conn
                .query(
                    "SELECT sequence, state, waiting_signal FROM snapshots WHERE actor_id = ?1",
                    libsql::params![actor_id as i64],
                )
                .await
                .ok()?;
            let row = rows.next().await.ok()??;
            let sequence: i64 = row.get(0).ok()?;
            let state_json: String = row.get(1).ok()?;
            let waiting_signal: Option<String> = row.get(2).ok()?;
            let state: HashMap<String, PersistedValue> = serde_json::from_str(&state_json).ok()?;
            Some(ActorSnapshot {
                actor_id,
                sequence: sequence as u64,
                state,
                waiting_signal,
            })
        })
    }

    fn append_journal(&mut self, actor_id: u64, entry: JournalEntry) -> io::Result<()> {
        let payload_json = serde_json::to_string(&entry.payload)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let conn = self.conn();
        self.rt.block_on(async {
            conn.execute(
                "INSERT INTO journal (actor_id, sequence, behavior_id, payload) VALUES (?1, ?2, ?3, ?4)",
                libsql::params![actor_id as i64, entry.sequence as i64, entry.behavior_id as i64, payload_json],
            ).await.map(|_| ()).map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))
        })
    }

    fn read_journal(&self, actor_id: u64) -> Vec<JournalEntry> {
        let conn = self.conn();
        self.rt.block_on(async {
            let mut rows = match conn
                .query(
                    "SELECT sequence, behavior_id, payload FROM journal
                 WHERE actor_id = ?1 ORDER BY sequence ASC",
                    libsql::params![actor_id as i64],
                )
                .await
            {
                Ok(r) => r,
                Err(_) => return Vec::new(),
            };
            let mut entries = Vec::new();
            loop {
                match rows.next().await {
                    Ok(Some(row)) => {
                        let seq: i64 = match row.get(0) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                        let bid: i64 = match row.get(1) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                        let payload_json: String = match row.get(2) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                        let payload: Vec<PersistedValue> = match serde_json::from_str(&payload_json)
                        {
                            Ok(p) => p,
                            Err(_) => continue,
                        };
                        entries.push(JournalEntry {
                            sequence: seq as u64,
                            behavior_id: bid as u16,
                            payload,
                        });
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            entries
        })
    }

    fn append_workflow_event(&mut self, actor_id: u64, event: WorkflowEvent) -> io::Result<()> {
        let event_json = serde_json::to_string(&event)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let conn = self.conn();
        self.rt.block_on(async {
            conn.execute(
                "INSERT INTO workflow_events (actor_id, sequence, event) VALUES (?1, ?2, ?3)",
                libsql::params![actor_id as i64, event.sequence() as i64, event_json],
            )
            .await
            .map(|_| ())
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))
        })
    }

    fn read_workflow_events(&self, actor_id: u64) -> Vec<WorkflowEvent> {
        let conn = self.conn();
        self.rt.block_on(async {
            let mut rows = match conn
                .query(
                    "SELECT event FROM workflow_events
                 WHERE actor_id = ?1 ORDER BY sequence ASC",
                    libsql::params![actor_id as i64],
                )
                .await
            {
                Ok(r) => r,
                Err(_) => return Vec::new(),
            };
            let mut events = Vec::new();
            loop {
                match rows.next().await {
                    Ok(Some(row)) => {
                        let event_json: String = match row.get(0) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                        if let Ok(event) = serde_json::from_str(&event_json) {
                            events.push(event);
                        }
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            events
        })
    }

    fn append_event(&mut self, actor_id: u64, entry: EventEntry) -> io::Result<()> {
        let args_json = serde_json::to_string(&entry.args)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let conn = self.conn();
        self.rt.block_on(async {
            conn.execute(
                "INSERT INTO events (actor_id, sequence, field_name, event_name, args) VALUES (?1, ?2, ?3, ?4, ?5)",
                libsql::params![actor_id as i64, entry.sequence as i64, entry.field_name, entry.event_name, args_json],
            ).await.map(|_| ()).map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))
        })
    }

    fn read_events(&self, actor_id: u64) -> Vec<EventEntry> {
        let conn = self.conn();
        self.rt.block_on(async {
            let mut rows = match conn
                .query(
                    "SELECT sequence, field_name, event_name, args FROM events
                 WHERE actor_id = ?1 ORDER BY sequence ASC",
                    libsql::params![actor_id as i64],
                )
                .await
            {
                Ok(r) => r,
                Err(_) => return Vec::new(),
            };
            let mut entries = Vec::new();
            loop {
                match rows.next().await {
                    Ok(Some(row)) => {
                        let seq: i64 = match row.get(0) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                        let field_name: String = match row.get(1) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                        let event_name: String = match row.get(2) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                        let args_json: String = match row.get(3) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                        let args: Vec<PersistedValue> = match serde_json::from_str(&args_json) {
                            Ok(p) => p,
                            Err(_) => continue,
                        };
                        entries.push(EventEntry {
                            sequence: seq as u64,
                            field_name,
                            event_name,
                            args,
                        });
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            entries
        })
    }

    fn latest_sequence(&self, actor_id: u64) -> u64 {
        let conn = self.conn();
        self.rt.block_on(async {
            let snapshot_seq: Option<i64> = async {
                let mut rows = conn.query(
                    "SELECT sequence FROM snapshots WHERE actor_id = ?1",
                    libsql::params![actor_id as i64],
                ).await.ok()?;
                let row = rows.next().await.ok()??;
                row.get(0).ok()
            }.await;
            let journal_seq: Option<i64> = async {
                let mut rows = conn.query(
                    "SELECT sequence FROM journal WHERE actor_id = ?1 ORDER BY sequence DESC LIMIT 1",
                    libsql::params![actor_id as i64],
                ).await.ok()?;
                let row = rows.next().await.ok()??;
                row.get(0).ok()
            }.await;
            let wf_event_seq: Option<i64> = async {
                let mut rows = conn.query(
                    "SELECT sequence FROM workflow_events WHERE actor_id = ?1 ORDER BY sequence DESC LIMIT 1",
                    libsql::params![actor_id as i64],
                ).await.ok()?;
                let row = rows.next().await.ok()??;
                row.get(0).ok()
            }.await;
            let event_seq: Option<i64> = async {
                let mut rows = conn.query(
                    "SELECT sequence FROM events WHERE actor_id = ?1 ORDER BY sequence DESC LIMIT 1",
                    libsql::params![actor_id as i64],
                ).await.ok()?;
                let row = rows.next().await.ok()??;
                row.get(0).ok()
            }.await;
            snapshot_seq.unwrap_or(0)
                .max(journal_seq.unwrap_or(0))
                .max(wf_event_seq.unwrap_or(0))
                .max(event_seq.unwrap_or(0)) as u64
        })
    }

    fn clear(&mut self, actor_id: u64) -> io::Result<()> {
        let conn = self.conn();
        self.rt.block_on(async {
            conn.execute(
                "DELETE FROM snapshots WHERE actor_id = ?1",
                libsql::params![actor_id as i64],
            )
            .await
            .map(|_| ())
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
            conn.execute(
                "DELETE FROM journal WHERE actor_id = ?1",
                libsql::params![actor_id as i64],
            )
            .await
            .map(|_| ())
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
            conn.execute(
                "DELETE FROM workflow_events WHERE actor_id = ?1",
                libsql::params![actor_id as i64],
            )
            .await
            .map(|_| ())
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
            conn.execute(
                "DELETE FROM events WHERE actor_id = ?1",
                libsql::params![actor_id as i64],
            )
            .await
            .map(|_| ())
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod json_file_store_tests {
    use super::*;

    /// Unique scratch dir per test (the suite runs tests in parallel, and a
    /// re-run must not see a previous run's leftover files).
    fn fresh_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "nulang_json_store_test_{}_{}",
            std::process::id(),
            tag
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn test_json_file_store_save_load_snapshot() {
        let dir = fresh_dir("snapshot");
        let mut store = JsonFileStore::new(&dir).unwrap();
        let mut state = HashMap::new();
        state.insert("count".to_string(), PersistedValue::Int(42));
        store
            .save_snapshot(ActorSnapshot {
                actor_id: 1,
                sequence: 3,
                state,
                waiting_signal: None,
            })
            .unwrap();

        let loaded = store.load_snapshot(1).unwrap();
        assert_eq!(loaded.actor_id, 1);
        assert_eq!(loaded.sequence, 3);
        assert_eq!(loaded.state.get("count"), Some(&PersistedValue::Int(42)));

        // The atomic (temp + rename) write must not leave its temp file behind.
        assert!(!store
            .snapshot_path(1)
            .with_file_name("snapshot.json.tmp")
            .exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_json_file_store_append_read_journal() {
        let dir = fresh_dir("journal");
        let mut store = JsonFileStore::new(&dir).unwrap();
        store
            .append_journal(
                1,
                JournalEntry {
                    sequence: 1,
                    behavior_id: 0,
                    payload: vec![PersistedValue::Int(10)],
                },
            )
            .unwrap();
        store
            .append_journal(
                1,
                JournalEntry {
                    sequence: 2,
                    behavior_id: 1,
                    payload: vec![PersistedValue::Int(20)],
                },
            )
            .unwrap();

        let journal = store.read_journal(1);
        assert_eq!(journal.len(), 2);
        assert_eq!(journal[0].sequence, 1);
        assert_eq!(journal[1].behavior_id, 1);
        assert_eq!(journal[1].payload, vec![PersistedValue::Int(20)]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_json_file_store_latest_sequence() {
        let dir = fresh_dir("latest_seq");
        let mut store = JsonFileStore::new(&dir).unwrap();
        store
            .save_snapshot(ActorSnapshot {
                actor_id: 1,
                sequence: 5,
                state: HashMap::new(),
                waiting_signal: None,
            })
            .unwrap();
        store
            .append_journal(
                1,
                JournalEntry {
                    sequence: 7,
                    behavior_id: 0,
                    payload: vec![],
                },
            )
            .unwrap();
        assert_eq!(store.latest_sequence(1), 7);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_json_file_store_clear() {
        let dir = fresh_dir("clear");
        let mut store = JsonFileStore::new(&dir).unwrap();
        store
            .save_snapshot(ActorSnapshot {
                actor_id: 1,
                sequence: 1,
                state: HashMap::new(),
                waiting_signal: None,
            })
            .unwrap();
        store
            .append_journal(
                1,
                JournalEntry {
                    sequence: 2,
                    behavior_id: 0,
                    payload: vec![],
                },
            )
            .unwrap();

        store.clear(1).unwrap();
        assert!(store.load_snapshot(1).is_none());
        assert!(store.read_journal(1).is_empty());
        assert_eq!(store.latest_sequence(1), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_json_file_store_persists_across_instances() {
        let dir = fresh_dir("persist");
        {
            let mut store = JsonFileStore::new(&dir).unwrap();
            let mut state = HashMap::new();
            state.insert("x".to_string(), PersistedValue::Float(1.5));
            store
                .save_snapshot(ActorSnapshot {
                    actor_id: 1,
                    sequence: 1,
                    state,
                    waiting_signal: None,
                })
                .unwrap();
            store
                .append_journal(
                    1,
                    JournalEntry {
                        sequence: 2,
                        behavior_id: 0,
                        payload: vec![PersistedValue::Bool(true)],
                    },
                )
                .unwrap();
        }

        {
            let store = JsonFileStore::new(&dir).unwrap();
            let snapshot = store.load_snapshot(1).unwrap();
            assert_eq!(snapshot.sequence, 1);
            assert_eq!(snapshot.state.get("x"), Some(&PersistedValue::Float(1.5)));
            let journal = store.read_journal(1);
            assert_eq!(journal.len(), 1);
            assert_eq!(journal[0].payload, vec![PersistedValue::Bool(true)]);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_json_file_store_corrupted_snapshot_loads_none() {
        let dir = fresh_dir("corrupt");
        let mut store = JsonFileStore::new(&dir).unwrap();
        store
            .save_snapshot(ActorSnapshot {
                actor_id: 1,
                sequence: 9,
                state: HashMap::new(),
                waiting_signal: None,
            })
            .unwrap();

        // Simulate a torn write (the pre-fix failure mode): truncate the
        // snapshot file mid-JSON. Recovery must degrade gracefully to `None`
        // (and log) rather than panic.
        let path = store.snapshot_path(1);
        fs::write(&path, "{\"actor_id\": 1, \"sequ").unwrap();
        assert!(store.load_snapshot(1).is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
