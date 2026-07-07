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
pub struct ActorSnapshot {
    pub actor_id: u64,
    pub sequence: u64,
    pub state: HashMap<String, PersistedValue>,
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
pub struct WorkflowEvent {
    pub sequence: u64,
    pub name: String,
    pub args: Vec<PersistedValue>,
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
            .and_then(|e| e.last().map(|ev| ev.sequence))
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
            .map(|e| e.sequence)
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

/// SQLite-backed persistence store.
///
/// Each actor gets one row in the `snapshots` table and zero or more rows in
/// the `journal` table ordered by sequence number. State and payloads are
/// serialized to JSON and stored as TEXT.
#[derive(Debug)]
pub struct SqliteStore {
    conn: std::sync::Mutex<rusqlite::Connection>,
    path: PathBuf,
}

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
                state TEXT NOT NULL
            )",
            [],
        )
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
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
                name TEXT NOT NULL,
                args TEXT NOT NULL,
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

impl PersistenceStore for SqliteStore {
    fn save_snapshot(&mut self, snapshot: ActorSnapshot) -> io::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let state_json = serde_json::to_string(&snapshot.state)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let tx = conn.transaction()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        tx.execute(
            "INSERT INTO snapshots (actor_id, sequence, state) VALUES (?1, ?2, ?3)
             ON CONFLICT(actor_id) DO UPDATE SET sequence=excluded.sequence, state=excluded.state",
            rusqlite::params![snapshot.actor_id as i64, snapshot.sequence as i64, state_json],
        )
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        tx.commit()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        Ok(())
    }

    fn load_snapshot(&self, actor_id: u64) -> Option<ActorSnapshot> {
        let conn = self.conn.lock().unwrap();
        let (sequence, state_json): (i64, String) = conn
            .query_row(
                "SELECT sequence, state FROM snapshots WHERE actor_id = ?1",
                [actor_id as i64],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok()?;
        let state: HashMap<String, PersistedValue> = serde_json::from_str(&state_json).ok()?;
        Some(ActorSnapshot {
            actor_id,
            sequence: sequence as u64,
            state,
        })
    }

    fn append_journal(&mut self, actor_id: u64, entry: JournalEntry) -> io::Result<()> {
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
        let args_json = serde_json::to_string(&event.args)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        conn.execute(
            "INSERT INTO workflow_events (actor_id, sequence, name, args) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![actor_id as i64, event.sequence as i64, event.name, args_json],
        )
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        Ok(())
    }

    fn read_workflow_events(&self, actor_id: u64) -> Vec<WorkflowEvent> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT sequence, name, args FROM workflow_events
             WHERE actor_id = ?1 ORDER BY sequence ASC",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = stmt.query_map([actor_id as i64], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        });
        match rows {
            Ok(iter) => iter
                .filter_map(|r| {
                    let (seq, name, args_json) = r.ok()?;
                    let args: Vec<PersistedValue> = serde_json::from_str(&args_json).ok()?;
                    Some(WorkflowEvent {
                        sequence: seq as u64,
                        name,
                        args,
                    })
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    fn latest_sequence(&self, actor_id: u64) -> u64 {
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM snapshots WHERE actor_id = ?1", [actor_id as i64])
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        conn.execute("DELETE FROM journal WHERE actor_id = ?1", [actor_id as i64])
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        conn.execute("DELETE FROM workflow_events WHERE actor_id = ?1", [actor_id as i64])
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        Ok(())
    }
}
