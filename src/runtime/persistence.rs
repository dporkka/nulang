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
        matches!(self, StateModel::Durable | StateModel::EventSourced | StateModel::Crdt)
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
    StepCompleted {
        sequence: u64,
        step_name: String,
    },
    /// A timer was set for a workflow.
    TimerSet {
        sequence: u64,
        name: String,
        duration_ms: u64,
    },
    /// A previously set timer fired.
    TimerFired {
        sequence: u64,
        name: String,
    },
    /// An external signal was delivered to the workflow.
    SignalReceived {
        sequence: u64,
        name: String,
        payload: Option<String>,
    },
    /// A saga step was compensated after failure.
    SagaCompensated {
        sequence: u64,
        step_name: String,
    },
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
    fn append_timer_fired(
        &mut self,
        actor_id: u64,
        sequence: u64,
        name: String,
    ) -> io::Result<()> {
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

    /// Highest sequence number known for the actor.
    fn latest_sequence(&self, actor_id: u64) -> u64;

    /// Remove all data for an actor.
    fn clear(&mut self, actor_id: u64) -> io::Result<()>;
}

/// In-memory persistence store. Useful for tests and ephemeral durable actors.
#[derive(Debug, Default, Clone)]
pub struct MemoryStore {
    snapshots: HashMap<u64, ActorSnapshot>,
    journals: HashMap<u64, Vec<JournalEntry>>,
    workflow_events: HashMap<u64, Vec<WorkflowEvent>>,
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
        self.workflow_events.entry(actor_id).or_default().push(event);
        Ok(())
    }

    fn read_workflow_events(&self, actor_id: u64) -> Vec<WorkflowEvent> {
        self.workflow_events.get(&actor_id).cloned().unwrap_or_default()
    }

    fn latest_sequence(&self, actor_id: u64) -> u64 {
        let snapshot_seq = self.snapshots.get(&actor_id).map(|s| s.sequence).unwrap_or(0);
        let journal_seq = self.journals
            .get(&actor_id)
            .and_then(|j| j.last().map(|e| e.sequence))
            .unwrap_or(0);
        let event_seq = self.workflow_events
            .get(&actor_id)
            .and_then(|e| e.last().map(|ev| ev.sequence()))
            .unwrap_or(0);
        snapshot_seq.max(journal_seq).max(event_seq)
    }

    fn clear(&mut self, actor_id: u64) -> io::Result<()> {
        self.snapshots.remove(&actor_id);
        self.journals.remove(&actor_id);
        self.workflow_events.remove(&actor_id);
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
}

impl PersistenceStore for JsonFileStore {
    fn save_snapshot(&mut self, snapshot: ActorSnapshot) -> io::Result<()> {
        let dir = self.actor_dir(snapshot.actor_id);
        fs::create_dir_all(&dir)?;
        let path = self.snapshot_path(snapshot.actor_id);
        let json = serde_json::to_string_pretty(&snapshot)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let mut file = fs::File::create(path)?;
        file.write_all(json.as_bytes())?;
        Ok(())
    }

    fn load_snapshot(&self, actor_id: u64) -> Option<ActorSnapshot> {
        let path = self.snapshot_path(actor_id);
        let data = fs::read_to_string(path).ok()?;
        serde_json::from_str(&data).ok()
    }

    fn append_journal(&mut self, actor_id: u64, entry: JournalEntry) -> io::Result<()> {
        let dir = self.actor_dir(actor_id);
        fs::create_dir_all(&dir)?;
        let path = self.journal_path(actor_id);
        let mut file = fs::OpenOptions::new().create(true).append(true).open(path)?;
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
        let mut file = fs::OpenOptions::new().create(true).append(true).open(path)?;
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

    fn latest_sequence(&self, actor_id: u64) -> u64 {
        let snapshot_seq = self.load_snapshot(actor_id).map(|s| s.sequence).unwrap_or(0);
        let journal_seq = self.read_journal(actor_id)
            .last()
            .map(|e| e.sequence)
            .unwrap_or(0);
        let event_seq = self.read_workflow_events(actor_id)
            .last()
            .map(|e| e.sequence())
            .unwrap_or(0);
        snapshot_seq.max(journal_seq).max(event_seq)
    }

    fn clear(&mut self, actor_id: u64) -> io::Result<()> {
        let dir = self.actor_dir(actor_id);
        if dir.exists() {
            fs::remove_dir_all(dir)?;
        }
        Ok(())
    }
}

/// Acquire the connection mutex, recovering the guard even if a previous
/// holder panicked.
///
/// A panic in one thread holding the connection lock must not cascade
/// panics into every other thread that touches the store, so poisoning is
/// ignored rather than propagated. But a panic could in principle occur
/// between `tx.execute` and `tx.commit()` in `save_snapshot`, leaving a
/// transaction open on the connection; recovering the guard alone doesn't
/// undo that. `rusqlite::Transaction`'s own `Drop` impl already issues a
/// `ROLLBACK` when a transaction is dropped without `commit()` (including
/// during unwind), so this is normally already safe — but as defense in
/// depth against any transaction that manages to outlive its `Transaction`
/// handle (e.g. a future bug that leaks one across a panic boundary), issue
/// a best-effort `ROLLBACK` whenever we actually recover from poisoning, so
/// the connection can't be left mid-transaction for whoever locks it next.
#[cfg(feature = "sqlite")]
fn lock_ignore_poison(m: &std::sync::Mutex<rusqlite::Connection>) -> std::sync::MutexGuard<'_, rusqlite::Connection> {
    match m.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            let guard = poisoned.into_inner();
            let _ = guard.execute_batch("ROLLBACK");
            guard
        }
    }
}

/// SQLite-backed persistence store.
///
/// Each actor gets one row in the `snapshots` table and zero or more rows in
/// the `journal` table ordered by sequence number. State and payloads are
/// serialized to JSON and stored as TEXT.
#[cfg(feature = "sqlite")]
#[derive(Debug)]
pub struct SqliteStore {
    conn: std::sync::Mutex<rusqlite::Connection>,
    path: PathBuf,
}

#[cfg(feature = "sqlite")]
impl SqliteStore {
    /// Open (or create) a SQLite persistence store at `path`.
    ///
    /// Pass `:memory:` for an ephemeral in-memory store.
    pub fn new<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let conn = rusqlite::Connection::open(&path)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS snapshots (
                actor_id INTEGER PRIMARY KEY,
                sequence INTEGER NOT NULL,
                state TEXT NOT NULL,
                waiting_signal TEXT
            )",
            [],
        )
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        // Migrate existing databases that were created without the
        // waiting_signal column.
        let _ = conn.execute(
            "ALTER TABLE snapshots ADD COLUMN waiting_signal TEXT",
            [],
        );
        conn.execute(
            "CREATE TABLE IF NOT EXISTS journal (
                actor_id INTEGER NOT NULL,
                sequence INTEGER NOT NULL,
                behavior_id INTEGER NOT NULL,
                payload TEXT NOT NULL,
                PRIMARY KEY (actor_id, sequence)
            )",
            [],
        )
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS workflow_events (
                actor_id INTEGER NOT NULL,
                sequence INTEGER NOT NULL,
                event TEXT NOT NULL,
                PRIMARY KEY (actor_id, sequence)
            )",
            [],
        )
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        Ok(SqliteStore {
            conn: std::sync::Mutex::new(conn),
            path,
        })
    }

    /// Open a new in-memory SQLite store.
    pub fn in_memory() -> io::Result<Self> {
        Self::new(":memory:")
    }

    /// Return the path this store was opened with.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(feature = "sqlite")]
impl PersistenceStore for SqliteStore {
    fn save_snapshot(&mut self, snapshot: ActorSnapshot) -> io::Result<()> {
        let mut conn = lock_ignore_poison(&self.conn);
        let state_json = serde_json::to_string(&snapshot.state)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let tx = conn.transaction()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        tx.execute(
            "INSERT INTO snapshots (actor_id, sequence, state, waiting_signal) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(actor_id) DO UPDATE SET sequence=excluded.sequence, state=excluded.state, waiting_signal=excluded.waiting_signal",
            rusqlite::params![snapshot.actor_id as i64, snapshot.sequence as i64, state_json, snapshot.waiting_signal.as_ref()],
        )
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        tx.commit()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        Ok(())
    }

    fn load_snapshot(&self, actor_id: u64) -> Option<ActorSnapshot> {
        let conn = lock_ignore_poison(&self.conn);
        let (sequence, state_json, waiting_signal): (i64, String, Option<String>) = conn
            .query_row(
                "SELECT sequence, state, waiting_signal FROM snapshots WHERE actor_id = ?1",
                [actor_id as i64],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .ok()?;
        let state: HashMap<String, PersistedValue> = serde_json::from_str(&state_json).ok()?;
        Some(ActorSnapshot {
            actor_id,
            sequence: sequence as u64,
            state,
            waiting_signal,
        })
    }

    fn append_journal(&mut self, actor_id: u64, entry: JournalEntry) -> io::Result<()> {
        let conn = lock_ignore_poison(&self.conn);
        let payload_json = serde_json::to_string(&entry.payload)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        conn.execute(
            "INSERT INTO journal (actor_id, sequence, behavior_id, payload) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![actor_id as i64, entry.sequence as i64, entry.behavior_id as i64, payload_json],
        )
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        Ok(())
    }

    fn read_journal(&self, actor_id: u64) -> Vec<JournalEntry> {
        let conn = lock_ignore_poison(&self.conn);
        let mut stmt = match conn.prepare(
            "SELECT sequence, behavior_id, payload FROM journal
             WHERE actor_id = ?1 ORDER BY sequence ASC",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = stmt.query_map([actor_id as i64], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        });
        match rows {
            Ok(iter) => iter
                .filter_map(|r| {
                    let (seq, bid, payload_json) = r.ok()?;
                    let payload: Vec<PersistedValue> = serde_json::from_str(&payload_json).ok()?;
                    Some(JournalEntry {
                        sequence: seq as u64,
                        behavior_id: bid as u16,
                        payload,
                    })
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    fn append_workflow_event(&mut self, actor_id: u64, event: WorkflowEvent) -> io::Result<()> {
        let conn = lock_ignore_poison(&self.conn);
        let event_json = serde_json::to_string(&event)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        conn.execute(
            "INSERT INTO workflow_events (actor_id, sequence, event) VALUES (?1, ?2, ?3)",
            rusqlite::params![actor_id as i64, event.sequence() as i64, event_json],
        )
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        Ok(())
    }

    fn read_workflow_events(&self, actor_id: u64) -> Vec<WorkflowEvent> {
        let conn = lock_ignore_poison(&self.conn);
        let mut stmt = match conn.prepare(
            "SELECT event FROM workflow_events
             WHERE actor_id = ?1 ORDER BY sequence ASC",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = stmt.query_map([actor_id as i64], |row| {
            Ok(row.get::<_, String>(0)?)
        });
        match rows {
            Ok(iter) => iter
                .filter_map(|r| {
                    let event_json = r.ok()?;
                    serde_json::from_str(&event_json).ok()
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    fn latest_sequence(&self, actor_id: u64) -> u64 {
        let conn = lock_ignore_poison(&self.conn);
        let snapshot_seq: Option<i64> = conn
            .query_row(
                "SELECT sequence FROM snapshots WHERE actor_id = ?1",
                [actor_id as i64],
                |row| row.get(0),
            )
            .ok();
        let journal_seq: Option<i64> = conn
            .query_row(
                "SELECT sequence FROM journal WHERE actor_id = ?1 ORDER BY sequence DESC LIMIT 1",
                [actor_id as i64],
                |row| row.get(0),
            )
            .ok();
        let event_seq: Option<i64> = conn
            .query_row(
                "SELECT sequence FROM workflow_events WHERE actor_id = ?1 ORDER BY sequence DESC LIMIT 1",
                [actor_id as i64],
                |row| row.get(0),
            )
            .ok();
        snapshot_seq.unwrap_or(0).max(journal_seq.unwrap_or(0)).max(event_seq.unwrap_or(0)) as u64
    }

    fn clear(&mut self, actor_id: u64) -> io::Result<()> {
        let conn = lock_ignore_poison(&self.conn);
        conn.execute("DELETE FROM snapshots WHERE actor_id = ?1", [actor_id as i64])
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        conn.execute("DELETE FROM journal WHERE actor_id = ?1", [actor_id as i64])
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        conn.execute("DELETE FROM workflow_events WHERE actor_id = ?1", [actor_id as i64])
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        Ok(())
    }
}

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use super::*;

    /// Regression test: recovering a poisoned connection lock must also
    /// clean up any transaction a panicking holder left open, or the very
    /// next `save_snapshot`'s `conn.transaction()` fails with "cannot start
    /// a transaction within a transaction" instead of the lock recovery
    /// being transparent to callers.
    #[test]
    fn test_lock_ignore_poison_rolls_back_dangling_transaction() {
        let store = SqliteStore::in_memory().unwrap();

        // Poison the mutex while a transaction is left open on the
        // connection (simulating a panic between `tx.execute` and
        // `tx.commit()`), bypassing `Transaction`'s own rollback-on-drop by
        // never constructing a `Transaction` value at all.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let conn = store.conn.lock().unwrap();
            conn.execute_batch("BEGIN").unwrap();
            panic!("simulated panic mid-transaction");
        }));
        assert!(result.is_err(), "the panic should have propagated");
        assert!(store.conn.is_poisoned(), "the mutex should now be poisoned");

        // Recovering the lock must leave the connection usable: a fresh
        // transaction must be startable, not blocked by the dangling BEGIN.
        let conn = lock_ignore_poison(&store.conn);
        conn.execute_batch("BEGIN").unwrap();
        conn.execute_batch("COMMIT").unwrap();
    }

    #[test]
    fn test_lock_ignore_poison_returns_normally_when_not_poisoned() {
        let store = SqliteStore::in_memory().unwrap();
        let conn = lock_ignore_poison(&store.conn);
        conn.execute_batch("SELECT 1").unwrap();
    }
}
