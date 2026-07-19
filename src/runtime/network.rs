//! Network transport layer for Nulang's distributed actor runtime.
//!
//! This module enables actors on different machines to send messages to each
//! other transparently over TCP. It defines a binary wire protocol, manages
//! connection pooling, and runs background threads for asynchronous I/O.
//!
//! # Architecture
//!
//! Each node runs a [`NetworkTransport`] that:
//! 1. Listens on a TCP socket for incoming connections from peer nodes.
//! 2. Maintains a pool of active [`TcpConnection`]s to remote nodes.
//! 3. Receives [`Packet`]s from peers and exposes them via [`receive`][NetworkTransport::receive].
//! 4. Sends [`Packet`]s to peers via an internal outgoing queue.
//!
//! # Wire Protocol
//!
//! Every packet on the wire is length-prefixed:
//! ```text
//! [0..4]   message length (u32, big-endian, includes this header)
//! [4..8]   magic: "NUL0"
//! [8]      packet type discriminant
//! [9..17]  sequence number (u64, big-endian)
//! [17..]   type-specific payload
//! ```
//!
//! A 16-byte versioned handshake is exchanged immediately after the TCP
//! connection is established, *before* either side starts sending framed
//! packets: `[magic "NUL0"][version u32][node_id u64]`. A peer whose wire
//! version does not match [`crate::format::constants::WIRE_VERSION`] is
//! refused, never silently reinterpreted. See `SPEC2.md` §"Format Stability".

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Imports from the rest of the crate
// ---------------------------------------------------------------------------

use super::cluster::{NodeGossip, NodeStatus};
use super::crdt_manager::{CrdtDeltaOp, CrdtOp};
use super::MessagePriority;
use super::NodeId;
use crate::vm::Value;


// ---------------------------------------------------------------------------
// TransportAddr — network address for TCP or Unix domain sockets
// ---------------------------------------------------------------------------

/// Address for the NUL0 protocol.  TCP is the default; Unix domain sockets
/// enable same-host eBPF sockmap redirection in NLC deployments.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TransportAddr {
    Tcp(SocketAddr),
    #[cfg(unix)]
    Unix(std::path::PathBuf),
}

impl TransportAddr {
    pub fn tcp(addr: SocketAddr) -> Self { TransportAddr::Tcp(addr) }
    #[cfg(unix)]
    pub fn unix(path: impl Into<std::path::PathBuf>) -> Self { TransportAddr::Unix(path.into()) }
}

impl std::fmt::Display for TransportAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportAddr::Tcp(a) => write!(f, "{}", a),
            #[cfg(unix)]
            TransportAddr::Unix(p) => write!(f, "unix:{}", p.display()),
        }
    }
}
// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Magic bytes that prefix every packet payload (after the length header).
/// The single source of truth is [`crate::format::constants::WIRE_MAGIC`];
/// this re-exports it for the packet framer.
const MAGIC: &[u8] = &crate::format::constants::WIRE_MAGIC;

/// Total size of the fixed packet header: 4 magic + 1 type + 8 seq.
const PACKET_HEADER_LEN: usize = 13;

/// TCP read / write timeout applied to every connection.
const IO_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the sender thread waits on the outgoing channel before
/// re-checking the shutdown flag.
const CHANNEL_RECV_TIMEOUT: Duration = Duration::from_millis(100);

// ---------------------------------------------------------------------------
// Versioned handshake helpers (magic + version + node_id = 16 bytes)
// ---------------------------------------------------------------------------

/// Write the 16-byte NUL0 versioned handshake to a stream.
fn write_handshake<W: Write>(w: &mut W, node_id: NodeId) -> io::Result<()> {
    w.write_all(&crate::format::constants::WIRE_MAGIC)?;
    w.write_all(&crate::format::constants::WIRE_VERSION.to_be_bytes())?;
    w.write_all(&node_id.0.to_be_bytes())?;
    w.flush()
}

/// Read the 16-byte NUL0 versioned handshake from a stream, validating the
/// magic and the wire protocol version. Returns the peer's node id. A
/// mismatched magic or version is a hard error: the connection is refused
/// rather than the peer's packets being reinterpreted under the wrong layout.
fn read_handshake<R: Read>(r: &mut R) -> io::Result<NodeId> {
    let mut buf = [0u8; crate::format::constants::WIRE_HANDSHAKE_LEN];
    r.read_exact(&mut buf)?;
    if &buf[0..4] != crate::format::constants::WIRE_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "wire handshake: bad magic, expected {:?}, got {:?}",
                crate::format::constants::WIRE_MAGIC,
                &buf[0..4]
            ),
        ));
    }
    let version = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
    if version != crate::format::constants::WIRE_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "wire handshake: peer speaks wire version {version}, this runtime speaks {}",
                crate::format::constants::WIRE_VERSION
            ),
        ));
    }
    let node_id = NodeId(u64::from_be_bytes([
        buf[8], buf[9], buf[10], buf[11], buf[12], buf[13], buf[14], buf[15],
    ]));
    Ok(node_id)
}

/// Maximum length (in bytes) of a single packet payload we are willing to
/// deserialize — a simple DoS protection.
const MAX_PACKET_LEN: u32 = 16 * 1024 * 1024; // 16 MiB

/// Capacity of the bounded internal channels.
const CHANNEL_CAPACITY: usize = 1024;

// Packet type discriminants.
const TYPE_ACTOR_MESSAGE: u8 = 0;
const TYPE_HEARTBEAT: u8 = 1;
const TYPE_ACK: u8 = 2;
const TYPE_SPAWN_REQUEST: u8 = 3;
const TYPE_SPAWN_RESPONSE: u8 = 4;
const TYPE_CRDT_SYNC: u8 = 5;
const TYPE_GOSSIP: u8 = 6;
const TYPE_CRDT_DELTA_SYNC: u8 = 7;

// ---------------------------------------------------------------------------
// NodeId
// ---------------------------------------------------------------------------

// NodeId is imported from super::cluster::NodeId

// ---------------------------------------------------------------------------
// Packet
// ---------------------------------------------------------------------------

/// A packet sent over the network between Nulang nodes.
#[derive(Debug, Clone, PartialEq)]
pub enum Packet {
    /// Send a message to an actor on the target node.
    ///
    /// The behavior is identified by **name**, not by id: behavior ids are
    /// per-actor-table indices and are meaningless across nodes. The
    /// receiving node resolves the name against the target actor's behavior
    /// table (the same rule local sends use in `Runtime::behavior_id_for`).
    ActorMessage {
        target_actor: u64,
        behavior_name: String,
        /// Optional BLAKE3 content hash of the expected behavior implementation.
        /// Set by the sender if the behavior has a known content hash in the
        /// sender's module; the receiver MAY verify it against the local
        /// behavior table during delivery (see process_network_packets).
        content_hash: Option<[u8; 32]>,
        payload: Vec<Value>,
        /// UTF-8 content for every `Value::string(id)` in `payload`: on the
        /// wire a string-id value indexes **this table**, never the sender's
        /// or receiver's constant pool (a pool id is meaningless across
        /// nodes). The sending runtime populates the table from the sender's
        /// module pool (`distributed::resolve_wire_strings`); the receiving
        /// runtime interns each entry into the target actor's module pool
        /// (`distributed::intern_wire_strings`).
        string_table: Vec<String>,
        sender_actor: u64,
        sender_node: NodeId,
        priority: MessagePriority,
    },

    /// Heartbeat / ping between nodes.
    Heartbeat {
        node_id: NodeId,
        timestamp: u64, // millis since epoch
    },

    /// Acknowledge receipt of a packet.
    Ack { packet_seq: u64 },

    /// Request to spawn an actor remotely.
    SpawnRequest {
        request_id: u64,
        behavior_name: String,
        /// Optional BLAKE3 content hash for cross-node behavior identity
        /// verification. The receiver MAY check this against the local
        /// `spawnable_behaviors` entry.
        content_hash: Option<[u8; 32]>,
        initial_state: Vec<(String, Value)>,
        bytecode: Option<Vec<u8>>,
    },

    /// Response to a spawn request.
    SpawnResponse {
        request_id: u64,
        actor_id: u64,
        success: bool,
    },

    /// CRDT synchronization packet.
    CrdtSync { ops: Vec<CrdtOp> },

    /// Delta-state CRDT synchronization packet.
    ///
    /// Each op is tagged as a delta (changes since the sender's last sync)
    /// or a full-state snapshot — see [`CrdtDeltaOp`]. Receivers merge
    /// deltas into entries they already hold and apply full-state ops like
    /// [`CrdtSync`](Packet::CrdtSync). The full-state `CrdtSync` packet
    /// remains available as the join/reset fallback.
    CrdtDeltaSync { ops: Vec<CrdtDeltaOp> },

    /// Cluster membership gossip.
    ///
    /// Carries the sender's (compact) membership view; the receiver merges
    /// it via [`ClusterState::merge_membership`](crate::runtime::cluster::ClusterState::merge_membership),
    /// where higher incarnation numbers win. This is what gives membership
    /// transitive propagation: a node relays what it knows, so a chain of
    /// pairwise seeds still converges to a full mesh.
    Gossip { members: Vec<NodeGossip> },
}

impl Packet {
    // ------------------------------------------------------------------
    // Public serialization API
    // ------------------------------------------------------------------

    /// Serialize the packet into bytes **without** the outer length prefix.
    ///
    /// The returned vector starts with [`MAGIC`], followed by the type
    /// discriminant, sequence number, and type-specific payload.
    pub fn to_bytes(&self, seq: u64) -> Vec<u8> {
        let mut buf = Vec::with_capacity(256);

        // Magic
        buf.extend_from_slice(MAGIC);

        // Type discriminant
        buf.push(self.discriminant());

        // Sequence number (big-endian)
        buf.extend_from_slice(&seq.to_be_bytes());

        // Payload
        self.write_payload(&mut buf);

        buf
    }

    /// Deserialize a packet from bytes (starting at the magic bytes).
    ///
    /// Returns `None` if the bytes are malformed or the discriminant is
    /// unknown.
    pub fn from_bytes(bytes: &[u8]) -> Option<(u64, Self)> {
        if bytes.len() < PACKET_HEADER_LEN {
            return None;
        }
        if &bytes[0..4] != MAGIC {
            return None;
        }

        let discriminant = bytes[4];
        let seq = u64::from_be_bytes([
            bytes[5], bytes[6], bytes[7], bytes[8], bytes[9], bytes[10], bytes[11], bytes[12],
        ]);

        let payload = &bytes[PACKET_HEADER_LEN..];
        let packet = match discriminant {
            TYPE_ACTOR_MESSAGE => Self::read_actor_message(payload)?,
            TYPE_HEARTBEAT => Self::read_heartbeat(payload)?,
            TYPE_ACK => Self::read_ack(payload)?,
            TYPE_SPAWN_REQUEST => Self::read_spawn_request(payload)?,
            TYPE_SPAWN_RESPONSE => Self::read_spawn_response(payload)?,
            TYPE_CRDT_SYNC => Self::read_crdt_sync(payload)?,
            TYPE_CRDT_DELTA_SYNC => Self::read_crdt_delta_sync(payload)?,
            TYPE_GOSSIP => Self::read_gossip(payload)?,
            _ => return None,
        };

        Some((seq, packet))
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    fn discriminant(&self) -> u8 {
        match self {
            Packet::ActorMessage { .. } => TYPE_ACTOR_MESSAGE,
            Packet::Heartbeat { .. } => TYPE_HEARTBEAT,
            Packet::Ack { .. } => TYPE_ACK,
            Packet::SpawnRequest { .. } => TYPE_SPAWN_REQUEST,
            Packet::SpawnResponse { .. } => TYPE_SPAWN_RESPONSE,
            Packet::CrdtSync { .. } => TYPE_CRDT_SYNC,
            Packet::CrdtDeltaSync { .. } => TYPE_CRDT_DELTA_SYNC,
            Packet::Gossip { .. } => TYPE_GOSSIP,
        }
    }

    fn write_payload(&self, buf: &mut Vec<u8>) {
        match self {
            Packet::ActorMessage {
                target_actor,
                behavior_name,
                content_hash,
                payload,
                string_table,
                sender_actor,
                sender_node,
                priority,
            } => {
                buf.extend_from_slice(&target_actor.to_be_bytes());
                write_string(buf, behavior_name);
                // content_hash: 1 byte flag + optional 32 bytes
                write_optional_hash(buf, content_hash);
                buf.extend_from_slice(&sender_actor.to_be_bytes());
                buf.extend_from_slice(&sender_node.0.to_be_bytes());
                buf.push(*priority as u8);
                buf.extend_from_slice(&(payload.len() as u32).to_be_bytes());
                for v in payload {
                    write_value(buf, v);
                }
                // String contents travel after the payload values; the
                // string-id values above index this table.
                buf.extend_from_slice(&(string_table.len() as u32).to_be_bytes());
                for s in string_table {
                    write_string(buf, s);
                }
            }
            Packet::Heartbeat { node_id, timestamp } => {
                buf.extend_from_slice(&node_id.0.to_be_bytes());
                buf.extend_from_slice(&timestamp.to_be_bytes());
            }
            Packet::Ack { packet_seq } => {
                buf.extend_from_slice(&packet_seq.to_be_bytes());
            }
            Packet::SpawnRequest {
                request_id,
                behavior_name,
                content_hash,
                initial_state,
                bytecode,
            } => {
                buf.extend_from_slice(&request_id.to_be_bytes());
                write_string(buf, behavior_name);
                // content_hash: 1 byte flag + optional 32 bytes
                write_optional_hash(buf, content_hash);
                buf.extend_from_slice(&(initial_state.len() as u32).to_be_bytes());
                for (key, value) in initial_state {
                    write_string(buf, key);
                    write_value(buf, value);
                }
                // Serialize optional bytecode: 0 length = None.
                match bytecode {
                    Some(bytes) => {
                        buf.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
                        buf.extend_from_slice(bytes);
                    }
                    None => {
                        buf.extend_from_slice(&0u32.to_be_bytes());
                    }
                }
            }
            Packet::SpawnResponse {
                request_id,
                actor_id,
                success,
            } => {
                buf.extend_from_slice(&request_id.to_be_bytes());
                buf.extend_from_slice(&actor_id.to_be_bytes());
                buf.push(if *success { 1 } else { 0 });
            }
            Packet::CrdtSync { ops } => {
                buf.extend_from_slice(&(ops.len() as u32).to_be_bytes());
                for op in ops {
                    buf.extend_from_slice(&op.to_bytes());
                }
            }
            Packet::CrdtDeltaSync { ops } => {
                buf.extend_from_slice(&(ops.len() as u32).to_be_bytes());
                for op in ops {
                    buf.extend_from_slice(&op.to_bytes());
                }
            }
            Packet::Gossip { members } => {
                buf.extend_from_slice(&(members.len() as u32).to_be_bytes());
                for m in members {
                    buf.extend_from_slice(&m.node_id.0.to_be_bytes());
                    write_addr(buf, &m.address);
                    buf.push(status_to_u8(m.status));
                    buf.extend_from_slice(&m.incarnation.to_be_bytes());
                }
            }
        }
    }

    // --- Deserialisation helpers for each variant ---------------------

    fn read_actor_message(payload: &[u8]) -> Option<Self> {
        if payload.len() < 12 {
            return None;
        }
        let target_actor = read_u64(payload, 0)?;
        let (behavior_name, name_len) = read_string(payload, 8)?;
        let mut offset = 8usize.checked_add(name_len)?;
        // content_hash: 1 byte flag + optional 32 bytes
        let (content_hash, hash_consumed) = read_optional_hash(payload, offset)?;
        offset = offset.checked_add(hash_consumed)?;
        if payload.len() < offset + 21 {
            return None;
        }
        let sender_actor = read_u64(payload, offset)?;
        let sender_node = NodeId(read_u64(payload, offset + 8)?);
        let priority = match payload.get(offset + 16).copied()? {
            0 => MessagePriority::System,
            1 => MessagePriority::Normal,
            2 => MessagePriority::Bulk,
            _ => return None,
        };
        let count = read_u32(payload, offset + 17)? as usize;
        offset = offset.checked_add(21)?;
        let mut values = Vec::with_capacity(count.min(1024));
        for _ in 0..count {
            let (v, consumed) = read_value(payload, offset)?;
            values.push(v);
            offset = offset.checked_add(consumed)?;
            if offset > payload.len() {
                return None;
            }
        }
        // String table: contents for the payload's string-id values.
        let table_count = read_u32(payload, offset)? as usize;
        offset = offset.checked_add(4)?;
        let mut string_table = Vec::with_capacity(table_count.min(1024));
        for _ in 0..table_count {
            let (s, consumed) = read_string(payload, offset)?;
            string_table.push(s);
            offset = offset.checked_add(consumed)?;
        }
        Some(Packet::ActorMessage {
            target_actor,
            behavior_name,
            content_hash,
            payload: values,
            string_table,
            sender_actor,
            sender_node,
            priority,
        })
    }

    fn read_heartbeat(payload: &[u8]) -> Option<Self> {
        if payload.len() < 16 {
            return None;
        }
        let node_id = NodeId(read_u64(payload, 0)?);
        let timestamp = read_u64(payload, 8)?;
        Some(Packet::Heartbeat { node_id, timestamp })
    }

    fn read_ack(payload: &[u8]) -> Option<Self> {
        let packet_seq = read_u64(payload, 0)?;
        Some(Packet::Ack { packet_seq })
    }

    fn read_spawn_request(payload: &[u8]) -> Option<Self> {
        if payload.len() < 8 {
            return None;
        }
        let request_id = read_u64(payload, 0)?;
        let (behavior_name, consumed) = read_string(payload, 8)?;
        let mut offset = 8 + consumed;
        // content_hash: 1 byte flag + optional 32 bytes
        let (content_hash, hash_consumed) = read_optional_hash(payload, offset)?;
        offset = offset.checked_add(hash_consumed)?;
        let count = read_u32(payload, offset)? as usize;
        offset += 4;
        let mut initial_state = Vec::with_capacity(count.min(256));
        for _ in 0..count {
            let (key, consumed_key) = read_string(payload, offset)?;
            offset = offset.checked_add(consumed_key)?;
            let (value, consumed_val) = read_value(payload, offset)?;
            offset = offset.checked_add(consumed_val)?;
            initial_state.push((key, value));
        }
        // Deserialize optional bytecode: 0 length = None.
        let bytecode_len = read_u32(payload, offset)? as usize;
        offset += 4;
        let bytecode = if bytecode_len > 0 {
            if offset + bytecode_len > payload.len() {
                return None;
            }
            Some(payload[offset..offset + bytecode_len].to_vec())
        } else {
            None
        };
        Some(Packet::SpawnRequest {
            request_id,
            behavior_name,
            content_hash,
            initial_state,
            bytecode,
        })
    }
    fn read_spawn_response(payload: &[u8]) -> Option<Self> {
        if payload.len() < 17 {
            return None;
        }
        let request_id = read_u64(payload, 0)?;
        let actor_id = read_u64(payload, 8)?;
        let success = payload.get(16).copied()? != 0;
        Some(Packet::SpawnResponse {
            request_id,
            actor_id,
            success,
        })
    }

    fn read_crdt_sync(payload: &[u8]) -> Option<Self> {
        if payload.len() < 4 {
            return None;
        }
        let count = read_u32(payload, 0)? as usize;
        let mut offset = 4usize;
        let mut ops = Vec::with_capacity(count.min(1024));
        for _ in 0..count {
            if offset >= payload.len() {
                return None;
            }
            // Each CrdtOp: [id:u64][type:u8][len:u32][payload]
            if offset + 13 > payload.len() {
                return None;
            }
            // Parse id + type + len manually to compute op byte length
            let op_payload_len = u32::from_be_bytes([
                payload[offset + 9],
                payload[offset + 10],
                payload[offset + 11],
                payload[offset + 12],
            ]) as usize;
            let total_op_len = 13 + op_payload_len;
            if offset + total_op_len > payload.len() {
                return None;
            }
            let op = CrdtOp::from_bytes(&payload[offset..offset + total_op_len])?;
            offset += total_op_len;
            ops.push(op);
        }
        Some(Packet::CrdtSync { ops })
    }

    fn read_crdt_delta_sync(payload: &[u8]) -> Option<Self> {
        if payload.len() < 4 {
            return None;
        }
        let count = read_u32(payload, 0)? as usize;
        let mut offset = 4usize;
        let mut ops = Vec::with_capacity(count.min(1024));
        for _ in 0..count {
            // Each CrdtDeltaOp: [is_delta:u8][id:u64][type:u8][len:u32][payload]
            if offset + 14 > payload.len() {
                return None;
            }
            // Parse flag + id + type + len manually to compute op byte length
            let op_payload_len = u32::from_be_bytes([
                payload[offset + 10],
                payload[offset + 11],
                payload[offset + 12],
                payload[offset + 13],
            ]) as usize;
            let total_op_len = 14 + op_payload_len;
            if offset + total_op_len > payload.len() {
                return None;
            }
            let op = CrdtDeltaOp::from_bytes(&payload[offset..offset + total_op_len])?;
            offset += total_op_len;
            ops.push(op);
        }
        Some(Packet::CrdtDeltaSync { ops })
    }

    fn read_gossip(payload: &[u8]) -> Option<Self> {
        if payload.len() < 4 {
            return None;
        }
        let count = read_u32(payload, 0)? as usize;
        let mut offset = 4usize;
        let mut members = Vec::with_capacity(count.min(1024));
        for _ in 0..count {
            // Each entry: [node_id:u64][addr][status:u8][incarnation:u64]
            if offset + 8 > payload.len() {
                return None;
            }
            let node_id = NodeId(read_u64(payload, offset)?);
            offset += 8;
            let (address, consumed) = read_addr(payload, offset)?;
            offset = offset.checked_add(consumed)?;
            if offset + 9 > payload.len() {
                return None;
            }
            let status = status_from_u8(*payload.get(offset)?)?;
            offset += 1;
            let incarnation = read_u64(payload, offset)?;
            offset += 8;
            members.push(NodeGossip {
                node_id,
                address,
                status,
                incarnation,
            });
        }
        Some(Packet::Gossip { members })
    }
}
// ---------------------------------------------------------------------------
// Value (de)serialization helpers
// ---------------------------------------------------------------------------

// Type tags for Value variants.
const VAL_INT: u8 = 0;
const VAL_FLOAT: u8 = 1;
const VAL_BOOL: u8 = 2;
const VAL_STRING: u8 = 3;
const VAL_UNIT: u8 = 4;
const VAL_NIL: u8 = 5;

/// Write a [`Value`] into `buf`.
fn write_value(buf: &mut Vec<u8>, v: &Value) {
    if let Some(i) = v.as_int() {
        buf.push(VAL_INT);
        buf.extend_from_slice(&i.to_be_bytes());
    } else if let Some(f) = v.as_float() {
        buf.push(VAL_FLOAT);
        buf.extend_from_slice(&f.to_be_bytes());
    } else if let Some(b) = v.as_bool() {
        buf.push(VAL_BOOL);
        buf.push(if b { 1 } else { 0 });
    } else if let Some(id) = v.as_string_id() {
        // The id indexes the enclosing packet's string table, not any
        // constant pool — see `Packet::ActorMessage::string_table`.
        buf.push(VAL_STRING);
        buf.extend_from_slice(&id.to_be_bytes());
    } else if v.is_unit() {
        buf.push(VAL_UNIT);
    } else if v.is_nil() {
        buf.push(VAL_NIL);
    } else {
        // Fall back to writing raw bits as float (for NaN floats or other tagged NaNs)
        buf.push(VAL_FLOAT);
        buf.extend_from_slice(&v.as_raw().to_be_bytes());
    }
}

/// A [`Value`] is wire-safe only if it can cross to another node without
/// silent corruption: int, float, bool, nil, or unit always qualify. A heap
/// pointer is process-local, so those are always rejected. A string-id is
/// safe only when `strings_ok` — i.e. the enclosing packet carries a string
/// table with the content (actor messages do; spawn requests do not).
fn value_is_wire_safe(v: &Value, strings_ok: bool) -> bool {
    !(v.is_ptr() || v.is_actor_ref() || v.is_closure())
        && (strings_ok || !v.is_string())
}

/// True if every payload [`Value`] carried by `packet` is wire-safe.
///
/// Only actor messages and spawn requests carry `Value`s; all other packet
/// kinds serialize plain scalars and are always safe to send. Actor-message
/// strings must additionally index the packet's string table — a string id
/// without a table entry is a dangling reference and is rejected. Spawn
/// requests keep strings rejected entirely: remotely-spawned actors run
/// native handlers and have no module pool to intern content into.
fn packet_payload_wire_safe(packet: &Packet) -> bool {
    match packet {
        Packet::ActorMessage {
            payload,
            string_table,
            ..
        } => payload.iter().all(|v| {
            value_is_wire_safe(v, true)
                && v
                    .as_string_id()
                    .map_or(true, |id| (id as usize) < string_table.len())
        }),
        Packet::SpawnRequest { initial_state, .. } => {
            initial_state.iter().all(|(_, v)| value_is_wire_safe(v, false))
        }
        _ => true,
    }
}

/// Read a [`Value`] from `bytes` starting at `offset`.
///
/// Returns `(Value, bytes_consumed)`.
fn read_value(bytes: &[u8], offset: usize) -> Option<(Value, usize)> {
    let tag = *bytes.get(offset)?;
    match tag {
        VAL_INT => {
            let v = read_i64(bytes, offset + 1)?;
            Some((Value::int(v), 1 + 8))
        }
        VAL_FLOAT => {
            let bits = read_u64(bytes, offset + 1)?;
            Some((Value::float(f64::from_bits(bits)), 1 + 8))
        }
        VAL_BOOL => {
            let b = *bytes.get(offset + 1)? != 0;
            Some((Value::bool(b), 1 + 1))
        }
        VAL_STRING => {
            let id = read_u32(bytes, offset + 1)?;
            Some((Value::string(id), 1 + 4))
        }
        VAL_UNIT => Some((Value::unit(), 1)),
        VAL_NIL => Some((Value::nil(), 1)),
        _ => None,
    }
}

/// Append a length-prefixed UTF-8 string.
fn write_string(buf: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    buf.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    buf.extend_from_slice(bytes);
}

/// Read a length-prefixed UTF-8 string.
///
/// Returns `(String, total_bytes_consumed)`.
fn read_string(bytes: &[u8], offset: usize) -> Option<(String, usize)> {
    let len = read_u32(bytes, offset)? as usize;
    let start = offset + 4;
    let end = start + len;
    if end > bytes.len() {
        return None;
    }
    let s = String::from_utf8(bytes[start..end].to_vec()).ok()?;
    Some((s, 4 + len))
}

/// Write an optional BLAKE3 content hash: 1-byte flag (0=None, 1=Some)
/// followed by 32 bytes when present.
fn write_optional_hash(buf: &mut Vec<u8>, hash: &Option<[u8; 32]>) {
    match hash {
        Some(h) => {
            buf.push(1);
            buf.extend_from_slice(h);
        }
        None => {
            buf.push(0);
        }
    }
}

/// Read an optional BLAKE3 content hash.
///
/// Returns `(Option<[u8; 32]>, bytes_consumed)`.
fn read_optional_hash(bytes: &[u8], offset: usize) -> Option<(Option<[u8; 32]>, usize)> {
    let flag = *bytes.get(offset)?;
    match flag {
        0 => Some((None, 1)),
        1 => {
            let start = offset + 1;
            let end = start + 32;
            if end > bytes.len() {
                return None;
            }
            let mut hash = [0u8; 32];
            hash.copy_from_slice(&bytes[start..end]);
            Some((Some(hash), 33))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// SocketAddr / NodeStatus (de)serialization helpers
// ---------------------------------------------------------------------------

/// Address family tags for [`write_addr`] / [`read_addr`].
const ADDR_IPV4: u8 = 4;
const ADDR_IPV6: u8 = 6;

/// Append a [`SocketAddr`] as `[family:u8][octets][port:u16]`.
fn write_addr(buf: &mut Vec<u8>, addr: &SocketAddr) {
    match addr {
        SocketAddr::V4(v4) => {
            buf.push(ADDR_IPV4);
            buf.extend_from_slice(&v4.ip().octets());
        }
        SocketAddr::V6(v6) => {
            buf.push(ADDR_IPV6);
            buf.extend_from_slice(&v6.ip().octets());
        }
    }
    buf.extend_from_slice(&addr.port().to_be_bytes());
}

/// Read a [`SocketAddr`] written by [`write_addr`].
///
/// Returns `(addr, bytes_consumed)`.
fn read_addr(bytes: &[u8], offset: usize) -> Option<(SocketAddr, usize)> {
    let family = *bytes.get(offset)?;
    let addr = match family {
        ADDR_IPV4 => {
            let octets: [u8; 4] = bytes.get(offset + 1..offset + 5)?.try_into().ok()?;
            let port = u16::from_be_bytes(bytes.get(offset + 5..offset + 7)?.try_into().ok()?);
            (
                SocketAddr::new(std::net::IpAddr::V4(octets.into()), port),
                1 + 4 + 2,
            )
        }
        ADDR_IPV6 => {
            let octets: [u8; 16] = bytes.get(offset + 1..offset + 17)?.try_into().ok()?;
            let port = u16::from_be_bytes(bytes.get(offset + 17..offset + 19)?.try_into().ok()?);
            (
                SocketAddr::new(std::net::IpAddr::V6(octets.into()), port),
                1 + 16 + 2,
            )
        }
        _ => return None,
    };
    Some(addr)
}

/// Map a [`NodeStatus`] to its wire byte.
fn status_to_u8(status: NodeStatus) -> u8 {
    match status {
        NodeStatus::Joining => 0,
        NodeStatus::Healthy => 1,
        NodeStatus::Suspicious => 2,
        NodeStatus::Failed => 3,
        NodeStatus::Leaving => 4,
    }
}

/// Inverse of [`status_to_u8`].
fn status_from_u8(b: u8) -> Option<NodeStatus> {
    match b {
        0 => Some(NodeStatus::Joining),
        1 => Some(NodeStatus::Healthy),
        2 => Some(NodeStatus::Suspicious),
        3 => Some(NodeStatus::Failed),
        4 => Some(NodeStatus::Leaving),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Little endian-agnostic integer readers / writers
// ---------------------------------------------------------------------------

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    let slice = bytes.get(offset..offset + 8)?;
    let arr: [u8; 8] = slice.try_into().ok()?;
    Some(u64::from_be_bytes(arr))
}

fn read_i64(bytes: &[u8], offset: usize) -> Option<i64> {
    let slice = bytes.get(offset..offset + 8)?;
    let arr: [u8; 8] = slice.try_into().ok()?;
    Some(i64::from_be_bytes(arr))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + 4)?;
    let arr: [u8; 4] = slice.try_into().ok()?;
    Some(u32::from_be_bytes(arr))
}

/// Acquire a mutex, recovering the guard even if a previous holder panicked.
///
/// Networking threads must keep running when one thread panics while holding
/// a shared lock; the data protected by these mutexes stays structurally
/// valid across panics, so poisoning is ignored rather than cascaded.
fn lock_ignore_poison<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

// ---------------------------------------------------------------------------
// TcpConnection
// ---------------------------------------------------------------------------

/// A single TCP connection to a remote node.
pub struct TcpConnection {
    pub node_id: NodeId,
    pub addr: SocketAddr,
    pub stream: TcpStream,
    pub last_activity: Instant,
}

impl TcpConnection {
    /// Write a framed packet (length-prefixed) to the stream.
    fn send_packet(&mut self, packet_bytes: &[u8]) -> io::Result<()> {
        let len = packet_bytes.len() as u32;
        self.stream.write_all(&len.to_be_bytes())?;
        self.stream.write_all(packet_bytes)?;
        self.stream.flush()?;
        self.last_activity = Instant::now();
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// IncomingPacket / OutgoingPacket
// ---------------------------------------------------------------------------

/// A packet that arrived from another node.
#[derive(Debug, Clone)]
pub struct IncomingPacket {
    pub from_node: NodeId,
    pub seq: u64,
    pub packet: Packet,
}

/// A packet to be sent to another node.
#[derive(Debug, Clone)]
pub struct OutgoingPacket {
    pub to_node: NodeId,
    pub to_addr: SocketAddr,
    pub packet: Packet,
}

// ---------------------------------------------------------------------------
// NetworkTransport
// ---------------------------------------------------------------------------

/// Manages all network connections for a Nulang node.
///
/// When created via [`bind`][TcpTransport::bind] the transport spawns
/// two long-lived background threads:
/// * a **listener** thread that accepts incoming TCP connections and
///   spawns a per-connection reader thread;
/// * a **sender** thread that dequeues [`OutgoingPacket`]s and writes
///   them to the appropriate TCP stream (connecting first if necessary).
pub trait NetworkTransport: Send {
    fn connect(&mut self, node_id: NodeId, addr: std::net::SocketAddr) -> std::io::Result<()>;
    fn send(&mut self, to_node: NodeId, to_addr: std::net::SocketAddr, packet: Packet);
    fn receive(&self) -> Vec<IncomingPacket>;
    fn node_id(&self) -> NodeId;
    fn listen_addr(&self) -> std::net::SocketAddr;
    fn disconnect(&mut self, node_id: NodeId);
    fn shutdown(&mut self);
    fn connection_count(&self) -> usize;
    fn connection_addr(&self, node_id: NodeId) -> Option<std::net::SocketAddr>;
}

impl NetworkTransport for Box<dyn NetworkTransport> {
    fn connect(&mut self, node_id: NodeId, addr: std::net::SocketAddr) -> std::io::Result<()> {
        (**self).connect(node_id, addr)
    }
    fn send(&mut self, to_node: NodeId, to_addr: std::net::SocketAddr, packet: Packet) {
        (**self).send(to_node, to_addr, packet)
    }
    fn receive(&self) -> Vec<IncomingPacket> {
        (**self).receive()
    }
    fn node_id(&self) -> NodeId { (**self).node_id() }
    fn listen_addr(&self) -> std::net::SocketAddr { (**self).listen_addr() }
    fn disconnect(&mut self, node_id: NodeId) { (**self).disconnect(node_id) }
    fn shutdown(&mut self) { (**self).shutdown() }
    fn connection_count(&self) -> usize { (**self).connection_count() }
    fn connection_addr(&self, node_id: NodeId) -> Option<std::net::SocketAddr> {
        (**self).connection_addr(node_id)
    }
}
pub struct TcpTransport {
    node_id: NodeId,
    listen_addr: SocketAddr,
    /// Active connections to other nodes.
    connections: Arc<Mutex<HashMap<NodeId, TcpConnection>>>,
    /// Channel endpoint used to receive packets from other nodes.
    incoming_rx: mpsc::Receiver<IncomingPacket>,
    /// Cloneable send side of the incoming channel, handed to reader
    /// threads spawned for dialled outbound connections.
    incoming_tx: mpsc::SyncSender<IncomingPacket>,
    /// Channel endpoint used to enqueue packets for transmission.
    outgoing_tx: mpsc::SyncSender<OutgoingPacket>,
    /// Background thread handles.
    threads: Arc<Mutex<Vec<JoinHandle<()>>>>,
    /// Sequence number counter for packets.
    next_seq: AtomicU64,
    /// Flag used to ask background threads to shut down.
    shutdown_flag: Arc<AtomicBool>,
}

impl TcpTransport {
    /// Create and bind a new network transport.
    ///
    /// The listener is bound to `addr`.  If `addr` has port `0` an
    /// ephemeral port is chosen by the OS and can be queried later via
    /// [`listen_addr`][NetworkTransport::listen_addr].
    ///
    /// Two background threads are started:
    /// 1. **Listener** – accepts incoming TCP connections.
    /// 2. **Sender** – drains the outgoing queue and writes to TCP streams.
    pub fn bind(addr: SocketAddr) -> io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        let listen_addr = listener.local_addr()?;
        let node_id = NodeId::new(&listen_addr);

        // Bounded channels.
        let (incoming_tx, incoming_rx) = mpsc::sync_channel(CHANNEL_CAPACITY);
        let (outgoing_tx, outgoing_rx) = mpsc::sync_channel(CHANNEL_CAPACITY);

        let connections: Arc<Mutex<HashMap<NodeId, TcpConnection>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let shutdown_flag = Arc::new(AtomicBool::new(false));
        let mut handles: Vec<JoinHandle<()>> = Vec::with_capacity(4);

        // ------------------------------------------------------------------
        // Listener thread
        // ------------------------------------------------------------------
        {
            let flag = Arc::clone(&shutdown_flag);
            let in_tx = incoming_tx.clone();
            let conns = Arc::clone(&connections);
            let local_id = node_id;
            let handle = thread::Builder::new()
                .name("nulang-net-listener".into())
                .spawn(move || {
                    listener_thread(listener, in_tx, conns, flag, local_id);
                })?;
            handles.push(handle);
        }

        // ------------------------------------------------------------------
        // Sender thread
        // ------------------------------------------------------------------
        {
            let flag = Arc::clone(&shutdown_flag);
            let conns = Arc::clone(&connections);
            let local_id = node_id;
            let in_tx = incoming_tx.clone();
            let handle = thread::Builder::new()
                .name("nulang-net-sender".into())
                .spawn(move || {
                    sender_thread(outgoing_rx, conns, flag, local_id, in_tx);
                })?;
            handles.push(handle);
        }

        Ok(TcpTransport {
            node_id,
            listen_addr,
            connections,
            incoming_rx,
            incoming_tx,
            outgoing_tx,
            threads: Arc::new(Mutex::new(handles)),
            next_seq: AtomicU64::new(1),
            shutdown_flag,
        })
    }

    /// Connect to a remote node.
    ///
    /// Establishes a TCP connection, performs the 8-byte node-id handshake,
    /// and registers the connection in the connection pool.
    pub fn connect(&mut self, node_id: NodeId, addr: SocketAddr) -> io::Result<()> {
        // Check if we already have a connection.
        {
            let conns = lock_ignore_poison(&self.connections);
            if conns.contains_key(&node_id) {
                return Ok(());
            }
        }

        // Bound the connect so one unreachable peer cannot stall this node:
        // `TcpStream::connect` would wait out the OS default (~2 min for a
        // blackholed peer).
        let mut stream = TcpStream::connect_timeout(&addr, IO_TIMEOUT)?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        stream.set_nodelay(true)?;

        // Handshake: send our versioned node_id, read theirs.
        write_handshake(&mut stream, self.node_id)?;
        let peer_id = read_handshake(&mut stream)?;

        // The peer should identify itself with the expected node_id.
        if peer_id != node_id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "handshake mismatch: expected {:?}, got {:?}",
                    node_id, peer_id
                ),
            ));
        }

        let conn = TcpConnection {
            node_id,
            addr,
            stream,
            last_activity: Instant::now(),
        };

        // Spawn a reader on a cloned handle so the link is fully duplex:
        // packets the peer writes to this socket (e.g. heartbeat replies)
        // are delivered just like packets on accepted inbound connections.
        let read_stream = conn.stream.try_clone()?;
        {
            let mut conns = lock_ignore_poison(&self.connections);
            conns.insert(node_id, conn);
        }
        let in_tx = self.incoming_tx.clone();
        let conns = Arc::clone(&self.connections);
        let flag = Arc::clone(&self.shutdown_flag);
        let _ = thread::Builder::new()
            .name(format!("nulang-net-reader-out-{}", addr.port()))
            .spawn(move || connection_read_loop(read_stream, node_id, in_tx, conns, flag));
        Ok(())
    }

    /// Send a packet to a remote node.
    ///
    /// A monotonically-increasing sequence number is attached automatically.
    /// The packet is enqueued on the outgoing channel; the background sender
    /// thread will establish a connection (if necessary) and write the
    /// packet to the wire.
    ///
    /// **Backpressure:** the outgoing channel is bounded
    /// ([`CHANNEL_CAPACITY`] packets). When it is full this call *blocks*
    /// until the sender thread drains a slot — this is deliberate
    /// backpressure toward the caller (typically the scheduler thread), not
    /// a silent drop. A packet is dropped only if the sender thread has
    /// already shut down (channel disconnected); that case is logged.
    pub fn send(&mut self, to_node: NodeId, to_addr: SocketAddr, packet: Packet) {
        // Reject payloads that cannot cross the wire losslessly. A heap
        // pointer is process-local and nil has no exact wire form; a string
        // id is only meaningful paired with the packet's string table.
        // Drop the packet loudly instead of silently mangling it.
        if !packet_payload_wire_safe(&packet) {
            eprintln!(
                "nulang-net: dropping packet to node {:?} (addr {}): payload value cannot cross the wire (heap pointer, nil, or string without content)",
                to_node, to_addr
            );
            return;
        }
        let seq = self.next_seq.fetch_add(1, Ordering::SeqCst);
        let outgoing = OutgoingPacket {
            to_node,
            to_addr,
            packet,
        };
        // Blocks on a full channel (backpressure). An error means the sender
        // thread has shut down and the packet cannot be delivered — log it
        // rather than dropping silently.
        if self.outgoing_tx.send(outgoing).is_err() {
            eprintln!(
                "nulang-net: dropping packet to node {:?} (addr {}): sender thread shut down",
                to_node, to_addr
            );
        }
        // The sequence number is part of the packet on the wire, but we
        // keep it in the transport for tracking.  For simplicity we
        // embed it directly into the bytes when the sender thread
        // serialises the packet, but we also store a "pending seq"
        // inside the OutgoingPacket by abusing the fact that the
        // Packet::to_bytes takes the seq.  The sender thread will
        // call to_bytes with the seq stored separately.
        //
        // To keep the API clean we stash the seq into the packet
        // by wrapping it in the channel as a tuple.
        //
        // Simpler approach: just resend with the seq baked in.
        let _ = seq;
    }

    /// Receive incoming packets (non-blocking).
    ///
    /// Returns all packets that have arrived since the last call.
    pub fn receive(&self) -> Vec<IncomingPacket> {
        let mut packets = Vec::new();
        loop {
            match self.incoming_rx.try_recv() {
                Ok(p) => packets.push(p),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => break,
            }
        }
        packets
    }

    /// Get this node's ID.
    pub fn node_id(&self) -> NodeId {
        self.node_id
    }

    /// Get the listen address.
    pub fn listen_addr(&self) -> SocketAddr {
        self.listen_addr
    }

    /// Disconnect from a remote node.
    ///
    /// Closes the TCP stream and removes the entry from the connection pool.
    pub fn disconnect(&mut self, node_id: NodeId) {
        let mut conns = lock_ignore_poison(&self.connections);
        if let Some(conn) = conns.remove(&node_id) {
            let _ = conn.stream.shutdown(Shutdown::Both);
        }
    }

    /// Shutdown the transport cleanly.
    ///
    /// Signals all background threads to stop, joins them, and closes
    /// every active TCP connection.
    pub fn shutdown(&mut self) {
        // Signal shutdown.
        self.shutdown_flag.store(true, Ordering::SeqCst);

        // Drop the outgoing sender so the sender thread wakes up and exits.
        let _ = std::mem::replace(&mut self.outgoing_tx, mpsc::sync_channel(1).0);

        // Close all connections so reader threads unblock.
        {
            let conns = lock_ignore_poison(&self.connections);
            for (_, conn) in conns.iter() {
                let _ = conn.stream.shutdown(Shutdown::Both);
            }
        }

        // Join all background threads.
        let handles: Vec<_> = {
            let mut guard = lock_ignore_poison(&self.threads);
            guard.drain(..).collect()
        };
        for h in handles {
            let _ = h.join();
        }
    }

    /// Get the number of active connections.
    pub fn connection_count(&self) -> usize {
        let conns = lock_ignore_poison(&self.connections);
        conns.len()
    }

    /// Look up the remote address of an active connection by node id.
    ///
    /// For connections we dialled this is the peer's listen address; for
    /// accepted inbound connections it is the peer's (ephemeral) source
    /// address. Either way it identifies a live link to the peer, which
    /// is enough for heartbeat-based membership discovery while the
    /// connection is open.
    pub fn connection_addr(&self, node_id: NodeId) -> Option<SocketAddr> {
        let conns = lock_ignore_poison(&self.connections);
        conns.get(&node_id).map(|conn| conn.addr)
    }
}

// ---------------------------------------------------------------------------
// Background thread implementations
// ---------------------------------------------------------------------------

/// Listener thread entry point.
///
/// Accepts incoming TCP connections.  For each accepted stream a new
/// "reader" thread is spawned that performs the handshake and then
/// enters a read-loop deserialising packets.
fn listener_thread(
    listener: TcpListener,
    incoming_tx: mpsc::SyncSender<IncomingPacket>,
    connections: Arc<Mutex<HashMap<NodeId, TcpConnection>>>,
    shutdown_flag: Arc<AtomicBool>,
    local_node_id: NodeId,
) {
    // Set a small accept timeout so we periodically check the shutdown flag.
    let _ = listener.set_nonblocking(true);

    loop {
        if shutdown_flag.load(Ordering::Relaxed) {
            break;
        }

        match listener.accept() {
            Ok((stream, addr)) => {
                let in_tx = incoming_tx.clone();
                let conns = Arc::clone(&connections);
                let flag = Arc::clone(&shutdown_flag);
                let _ = thread::Builder::new()
                    .name(format!("nulang-net-reader-{}", addr.port()))
                    .spawn(move || {
                        connection_reader(stream, addr, in_tx, conns, flag, local_node_id);
                    });
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => {
                // Listener socket broken — time to exit.
                break;
            }
        }
    }
}

/// Per-connection reader thread.
///
/// 1. Sends our node-id (8 bytes).
/// 2. Reads the peer's node-id (8 bytes).
/// 3. Registers the connection.
/// 4. Reads framed packets in a loop until disconnect or shutdown.
fn connection_reader(
    mut stream: TcpStream,
    addr: SocketAddr,
    incoming_tx: mpsc::SyncSender<IncomingPacket>,
    connections: Arc<Mutex<HashMap<NodeId, TcpConnection>>>,
    shutdown_flag: Arc<AtomicBool>,
    local_node_id: NodeId,
) {
    // Set timeouts.
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_nodelay(true);

    // --- Handshake: send local node_id, read remote node_id ---------------
    if write_handshake(&mut stream, local_node_id).is_err() {
        return;
    }

    let peer_id = match read_handshake(&mut stream) {
        Ok(id) => id,
        Err(_) => return,
    };

    // Register the connection.
    {
        let mut conns = lock_ignore_poison(&connections);
        conns.insert(
            peer_id,
            TcpConnection {
                node_id: peer_id,
                addr,
                stream: stream
                    .try_clone()
                    .expect("TcpStream::try_clone should succeed"),
                last_activity: Instant::now(),
            },
        );
    }

    connection_read_loop(stream, peer_id, incoming_tx, connections, shutdown_flag);
}

/// Read framed packets from `stream` until disconnect or shutdown, then
/// remove the peer from the connection pool.
///
/// Shared by the listener-side reader (accepted inbound connections) and
/// the reader spawned for dialled outbound connections, so every TCP
/// link is read exactly once regardless of which side initiated it.
fn connection_read_loop(
    mut stream: TcpStream,
    peer_id: NodeId,
    incoming_tx: mpsc::SyncSender<IncomingPacket>,
    connections: Arc<Mutex<HashMap<NodeId, TcpConnection>>>,
    shutdown_flag: Arc<AtomicBool>,
) {
    loop {
        if shutdown_flag.load(Ordering::Relaxed) {
            break;
        }

        // Read 4-byte length prefix.
        let mut len_buf = [0u8; 4];
        match stream.read_exact(&mut len_buf) {
            Ok(()) => {}
            Err(_) => break, // Disconnect or timeout.
        }
        let len = u32::from_be_bytes(len_buf);
        if len == 0 || len > MAX_PACKET_LEN {
            break; // Protocol error or DoS.
        }

        // Read payload.
        let mut payload = vec![0u8; len as usize];
        match stream.read_exact(&mut payload) {
            Ok(()) => {}
            Err(_) => break,
        }

        // Update activity timestamp.
        {
            let mut conns = lock_ignore_poison(&connections);
            if let Some(conn) = conns.get_mut(&peer_id) {
                conn.last_activity = Instant::now();
            }
        }

        if let Some((seq, packet)) = Packet::from_bytes(&payload) {
            let incoming = IncomingPacket {
                from_node: peer_id,
                seq,
                packet,
            };
            if incoming_tx.send(incoming).is_err() {
                // Channel disconnected — the transport is shutting down.
                break;
            }
        }
    }

    // Clean up: remove connection from pool.
    {
        let mut conns = lock_ignore_poison(&connections);
        conns.remove(&peer_id);
    }
    let _ = stream.shutdown(Shutdown::Both);
}

/// Sender thread entry point.
///
/// Drains the outgoing queue, looks up (or creates) the TCP connection
/// for each packet, and writes the framed bytes.
fn sender_thread(
    outgoing_rx: mpsc::Receiver<OutgoingPacket>,
    connections: Arc<Mutex<HashMap<NodeId, TcpConnection>>>,
    shutdown_flag: Arc<AtomicBool>,
    local_node_id: NodeId,
    incoming_tx: mpsc::SyncSender<IncomingPacket>,
) {
    // We keep a local sequence counter so we can embed it into the bytes.
    let mut next_seq: u64 = 1;

    loop {
        if shutdown_flag.load(Ordering::Relaxed) && outgoing_rx.try_recv().is_err() {
            break;
        }

        let outgoing = match outgoing_rx.recv_timeout(CHANNEL_RECV_TIMEOUT) {
            Ok(p) => p,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };

        // Look up connection.
        let mut needs_connect = false;
        {
            let conns = lock_ignore_poison(&connections);
            if !conns.contains_key(&outgoing.to_node) {
                needs_connect = true;
            }
        }

        // Establish connection if missing.
        if needs_connect {
            if let Err(e) = connect_in_sender(
                &connections,
                &incoming_tx,
                &shutdown_flag,
                local_node_id,
                outgoing.to_node,
                outgoing.to_addr,
            ) {
                eprintln!(
                    "[nulang-net] Failed to connect to {:?} at {}: {}",
                    outgoing.to_node, outgoing.to_addr, e
                );
                continue;
            }
        }

        // Send the packet.
        let seq = next_seq;
        next_seq = next_seq.wrapping_add(1);
        let bytes = outgoing.packet.to_bytes(seq);

        let result = {
            let mut conns = lock_ignore_poison(&connections);
            if let Some(conn) = conns.get_mut(&outgoing.to_node) {
                conn.send_packet(&bytes)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::NotConnected,
                    "connection disappeared",
                ))
            }
        };

        if let Err(e) = result {
            eprintln!(
                "[nulang-net] Send to {:?} failed: {}; removing connection",
                outgoing.to_node, e
            );
            let mut conns = lock_ignore_poison(&connections);
            if let Some(conn) = conns.remove(&outgoing.to_node) {
                let _ = conn.stream.shutdown(Shutdown::Both);
            }
        }
    }
}

/// Establish a TCP connection from inside the sender thread.
///
/// This is a best-effort connect that performs the 8-byte handshake.
/// A reader thread is spawned on a cloned handle so the link is fully
/// duplex: without it, a node that only ever dials out (e.g. a cluster
/// joiner) could never receive packets over the connection it
/// established, and heartbeat replies from its seed would be lost.
fn connect_in_sender(
    connections: &Arc<Mutex<HashMap<NodeId, TcpConnection>>>,
    incoming_tx: &mpsc::SyncSender<IncomingPacket>,
    shutdown_flag: &Arc<AtomicBool>,
    local_node_id: NodeId,
    node_id: NodeId,
    addr: SocketAddr,
) -> io::Result<()> {
    // Bound the connect: the single sender thread serialises every peer's
    // traffic, so an unreachable peer must not block all sends for the OS
    // default timeout (~2 min).
    let mut stream = TcpStream::connect_timeout(&addr, IO_TIMEOUT)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    stream.set_nodelay(true)?;

    // Handshake.
    write_handshake(&mut stream, local_node_id)?;
    let peer_id = read_handshake(&mut stream)?;

    if peer_id != node_id {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "node id handshake mismatch in sender connect",
        ));
    }

    let read_stream = stream.try_clone()?;
    {
        let mut conns = lock_ignore_poison(connections);
        conns.insert(
            node_id,
            TcpConnection {
                node_id,
                addr,
                stream,
                last_activity: Instant::now(),
            },
        );
    }
    let in_tx = incoming_tx.clone();
    let conns = Arc::clone(connections);
    let flag = Arc::clone(shutdown_flag);
    let _ = thread::Builder::new()
        .name(format!("nulang-net-reader-out-{}", addr.port()))
        .spawn(move || connection_read_loop(read_stream, node_id, in_tx, conns, flag));
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::super::crdt_manager::{CrdtId, CrdtType};
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};
    use std::thread::sleep;

    // ------------------------------------------------------------------
    // 1. NodeId hashing
    // ------------------------------------------------------------------
    #[test]
    fn test_node_id_from_addr() {
        let addr1 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 9000);
        let addr2 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 9001);
        let addr3 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 9000);

        let id1 = NodeId::new(&addr1);
        let id2 = NodeId::new(&addr2);
        let id3 = NodeId::new(&addr3);

        assert_eq!(id1, id3, "same address must produce same NodeId");
        assert_ne!(
            id1, id2,
            "different addresses must produce different NodeId"
        );
        assert_ne!(id1.0, 0, "NodeId must not be zero");
    }

    // ------------------------------------------------------------------
    // 2. ActorMessage roundtrip
    // ------------------------------------------------------------------
    #[test]
    fn test_packet_serialize_deserialize_actor_message() {
        let packet = Packet::ActorMessage {
            target_actor: 42,
            behavior_name: "handle_msg".to_string(),
            content_hash: None,
            payload: vec![Value::int(123), Value::string(456)],
            string_table: vec![],
            sender_actor: 99,
            sender_node: NodeId(0xDEAD_BEEF_CAFE_BABE),
            priority: MessagePriority::Normal,
        };

        let bytes = packet.to_bytes(0x1234);
        let (seq, decoded) = Packet::from_bytes(&bytes).expect("deserialization failed");

        assert_eq!(seq, 0x1234);
        assert_eq!(decoded, packet);
    }

    // ------------------------------------------------------------------
    // 2b. ActorMessage string table roundtrip
    // ------------------------------------------------------------------
    #[test]
    fn test_packet_actor_message_string_table_roundtrip() {
        let packet = Packet::ActorMessage {
            target_actor: 7,
            behavior_name: "store".to_string(),
            content_hash: None,
            payload: vec![Value::string(0), Value::string(1), Value::string(0)],
            string_table: vec!["hello".to_string(), "wörld ✓".to_string()],
            sender_actor: 3,
            sender_node: NodeId(0x1111_2222_3333_4444),
            priority: MessagePriority::Normal,
        };

        let bytes = packet.to_bytes(77);
        let (seq, decoded) =
            Packet::from_bytes(&bytes).expect("actor message deserialization failed");

        assert_eq!(seq, 77);
        assert_eq!(decoded, packet);
    }

    #[test]
    fn test_packet_actor_message_rejects_truncated_string_table() {
        let packet = Packet::ActorMessage {
            target_actor: 7,
            behavior_name: "store".to_string(),
            content_hash: None,
            payload: vec![Value::string(0)],
            string_table: vec!["hello".to_string()],
            sender_actor: 3,
            sender_node: NodeId(1),
            priority: MessagePriority::Normal,
        };
        let bytes = packet.to_bytes(1);
        // Chop the string table in half: the declared count/content no
        // longer fits, so deserialization must fail cleanly (no panic).
        let truncated = &bytes[..bytes.len() - 3];
        assert!(Packet::from_bytes(truncated).is_none());
        // A packet cut off right after the payload values (before the
        // table count: 4 bytes count + 4 bytes len + 5 bytes "hello") is
        // rejected too.
        let values_end = bytes.len() - 13;
        assert!(Packet::from_bytes(&bytes[..values_end]).is_none());
    }

    // ------------------------------------------------------------------
    // 3b. Gossip roundtrip
    // ------------------------------------------------------------------
    #[test]
    fn test_packet_serialize_deserialize_gossip() {
        let packet = Packet::Gossip {
            members: vec![
                NodeGossip {
                    node_id: NodeId(0x1111_2222_3333_4444),
                    address: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 9100),
                    status: NodeStatus::Healthy,
                    incarnation: 7,
                },
                NodeGossip {
                    node_id: NodeId(0xAAAA_BBBB_CCCC_DDDD),
                    address: SocketAddr::new(IpAddr::V6("::1".parse().unwrap()), 49152),
                    status: NodeStatus::Suspicious,
                    incarnation: u64::MAX,
                },
                NodeGossip {
                    node_id: NodeId(1),
                    address: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)), 0),
                    status: NodeStatus::Joining,
                    incarnation: 0,
                },
            ],
        };

        let bytes = packet.to_bytes(99);
        let (seq, decoded) = Packet::from_bytes(&bytes).expect("gossip deserialization failed");

        assert_eq!(seq, 99);
        assert_eq!(decoded, packet);
    }

    #[test]
    fn test_packet_gossip_rejects_truncated_payload() {
        // A header followed by a truncated entry must not panic and must
        // fail cleanly.
        let packet = Packet::Gossip {
            members: vec![NodeGossip {
                node_id: NodeId(42),
                address: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 9100),
                status: NodeStatus::Healthy,
                incarnation: 3,
            }],
        };
        let bytes = packet.to_bytes(1);
        // Keep the header + count, chop the entry in half.
        let truncated = &bytes[..bytes.len() - 5];
        assert!(Packet::from_bytes(truncated).is_none());
    }

    // ------------------------------------------------------------------
    // 3d. CRDT delta sync roundtrip
    // ------------------------------------------------------------------
    #[test]
    fn test_packet_serialize_deserialize_crdt_delta_sync() {
        let full_op = CrdtDeltaOp {
            op: CrdtOp {
                crdt_id: CrdtId(7),
                crdt_type: CrdtType::GCounter,
                payload: vec![0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0],
            },
            is_delta: false,
        };
        let delta_op = CrdtDeltaOp {
            op: CrdtOp {
                crdt_id: CrdtId(7),
                crdt_type: CrdtType::GCounter,
                payload: vec![1, 2, 3],
            },
            is_delta: true,
        };
        let packet = Packet::CrdtDeltaSync {
            ops: vec![full_op, delta_op],
        };

        let bytes = packet.to_bytes(42);
        let (seq, decoded) =
            Packet::from_bytes(&bytes).expect("crdt delta sync deserialization failed");

        assert_eq!(seq, 42);
        assert_eq!(decoded, packet);
    }

    #[test]
    fn test_packet_crdt_delta_sync_rejects_truncated_payload() {
        let packet = Packet::CrdtDeltaSync {
            ops: vec![CrdtDeltaOp {
                op: CrdtOp {
                    crdt_id: CrdtId(1),
                    crdt_type: CrdtType::GSet,
                    payload: vec![0xAB; 8],
                },
                is_delta: true,
            }],
        };
        let bytes = packet.to_bytes(1);
        // Keep the header + count, chop the op in half.
        let truncated = &bytes[..bytes.len() - 3];
        assert!(Packet::from_bytes(truncated).is_none());
    }

    // ------------------------------------------------------------------
    // 3c. Spawn request/response roundtrips
    // ------------------------------------------------------------------
    #[test]
    fn test_packet_serialize_deserialize_spawn_response() {
        let packet = Packet::SpawnResponse {
            request_id: 0xDEAD_BEEF,
            actor_id: 424242,
            success: true,
        };
        let bytes = packet.to_bytes(3);
        let (seq, decoded) = Packet::from_bytes(&bytes).unwrap();
        assert_eq!(seq, 3);
        assert_eq!(decoded, packet);
    }

    // ------------------------------------------------------------------
    // 3. Heartbeat roundtrip
    // ------------------------------------------------------------------
    #[test]
    fn test_packet_serialize_deserialize_heartbeat() {
        let packet = Packet::Heartbeat {
            node_id: NodeId(0xABCD),
            timestamp: 1_700_000_000_000,
        };

        let bytes = packet.to_bytes(7);
        let (seq, decoded) = Packet::from_bytes(&bytes).unwrap();

        assert_eq!(seq, 7);
        assert_eq!(decoded, packet);
    }

    // ------------------------------------------------------------------
    // 4. Int value roundtrip
    // ------------------------------------------------------------------
    #[test]
    fn test_value_serialization_int() {
        let v = Value::int(-42_000_000_000_i64);
        let mut buf = Vec::new();
        write_value(&mut buf, &v);

        let (decoded, consumed) = read_value(&buf, 0).unwrap();
        assert_eq!(consumed, 9); // 1 tag + 8 bytes
        assert_eq!(decoded, v);
    }

    // ------------------------------------------------------------------
    // 5. String value roundtrip
    // ------------------------------------------------------------------
    #[test]
    fn test_value_serialization_string() {
        let v = Value::string(42);
        let mut buf = Vec::new();
        write_value(&mut buf, &v);

        let (decoded, consumed) = read_value(&buf, 0).unwrap();
        assert_eq!(consumed, 5); // 1 tag + 4 bytes
        assert_eq!(decoded, v);
    }

    // ------------------------------------------------------------------
    // 6. Mixed payload roundtrip
    // ------------------------------------------------------------------
    #[test]
    fn test_value_serialization_complex() {
        let values = vec![
            Value::int(0),
            Value::int(-1),
            Value::int(i64::MAX),
            Value::int(i64::MIN),
            Value::float(std::f64::consts::PI),
            Value::float(f64::NAN),
            Value::float(f64::INFINITY),
            Value::bool(true),
            Value::bool(false),
            Value::string(0),
            Value::string(1),
            Value::string(999),
            Value::unit(),
        ];

        for v in &values {
            let mut buf = Vec::new();
            write_value(&mut buf, v);
            let (decoded, _) = read_value(&buf, 0).unwrap();

            // For NaN, we need to compare bits because NaN != NaN.
            match (v.as_float(), decoded.as_float()) {
                (Some(a), Some(b)) if a.is_nan() && b.is_nan() => {}
                _ => assert_eq!(decoded, *v, "roundtrip failed for {:?}", v),
            }
        }
    }

    // ------------------------------------------------------------------
    // 7. Transport bind
    // ------------------------------------------------------------------
    #[test]
    fn test_transport_bind() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0);
        let mut transport = TcpTransport::bind(addr).expect("bind failed");

        assert_eq!(
            transport.listen_addr().ip(),
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))
        );
        assert_ne!(
            transport.listen_addr().port(),
            0,
            "ephemeral port must be assigned"
        );
        assert_eq!(transport.connection_count(), 0);
        assert_eq!(transport.node_id(), NodeId::new(&transport.listen_addr()));

        transport.shutdown();
    }

    // ------------------------------------------------------------------
    // 8. Two transports can connect
    // ------------------------------------------------------------------
    #[test]
    fn test_transport_connect() {
        let addr_a = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0);
        let mut transport_a = TcpTransport::bind(addr_a).unwrap();

        let addr_b = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0);
        let mut transport_b = TcpTransport::bind(addr_b).unwrap();

        let addr_b_actual = transport_b.listen_addr();
        let node_b_id = transport_b.node_id();

        // A connects to B.
        transport_a
            .connect(node_b_id, addr_b_actual)
            .expect("connect failed");

        // Give the listener thread a moment to accept and handshake.
        sleep(Duration::from_millis(100));

        // B should have an incoming connection from A.
        // (B does not explicitly connect back — the TCP connection is
        //  bidirectional, but B only learns about A when A sends a packet.
        //  The connection is stored in A's pool; B also stores it when
        //  the reader thread accepts it.)
        assert!(
            transport_a.connection_count() >= 1,
            "transport A should have at least one connection"
        );

        transport_a.shutdown();
        transport_b.shutdown();
    }

    // ------------------------------------------------------------------
    // 9. Send packet and receive on the other side
    // ------------------------------------------------------------------
    #[test]
    fn test_transport_send_receive() {
        let addr_a = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0);
        let mut transport_a = TcpTransport::bind(addr_a).unwrap();

        let addr_b = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0);
        let mut transport_b = TcpTransport::bind(addr_b).unwrap();

        let addr_b_actual = transport_b.listen_addr();
        let node_b_id = transport_b.node_id();

        // A connects to B.
        transport_a.connect(node_b_id, addr_b_actual).unwrap();
        sleep(Duration::from_millis(100));

        // A sends a packet to B.
        let packet = Packet::Heartbeat {
            node_id: transport_a.node_id(),
            timestamp: 1_700_000_000,
        };
        transport_a.send(node_b_id, addr_b_actual, packet.clone());

        // B should eventually receive it.
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut received = Vec::new();
        while Instant::now() < deadline && received.is_empty() {
            received = transport_b.receive();
            if received.is_empty() {
                sleep(Duration::from_millis(50));
            }
        }

        assert!(
            !received.is_empty(),
            "transport B should have received the heartbeat"
        );
        assert_eq!(received[0].from_node, transport_a.node_id());
        assert_eq!(received[0].packet, packet);

        transport_a.shutdown();
        transport_b.shutdown();
    }

    // ------------------------------------------------------------------
    // 9b. Non-scalar payloads are rejected at send time
    // ------------------------------------------------------------------
    #[test]
    fn test_value_wire_safety_classification() {
        // Scalars round-trip exactly and are safe to send. Strings are safe
        // only where the packet can carry their content (`strings_ok`).
        assert!(value_is_wire_safe(&Value::int(1), false));
        assert!(value_is_wire_safe(&Value::float(2.5), false));
        assert!(value_is_wire_safe(&Value::bool(true), false));
        assert!(value_is_wire_safe(&Value::unit(), false));
        assert!(value_is_wire_safe(&Value::string(7), true));
        assert!(!value_is_wire_safe(&Value::string(7), false));

        // Heap/tagged values (except nil) would arrive corrupted on the
        // receiving node, so they must always be rejected. Nil is now
        // wire-safe (VAL_NIL tag).
        assert!(!value_is_wire_safe(&Value::ptr(std::ptr::null_mut()), true));
        assert!(!value_is_wire_safe(&Value::actor_ref(9), true));
        assert!(!value_is_wire_safe(&Value::closure(3), true));
        assert!(value_is_wire_safe(&Value::nil(), true));

        // Packet-level classification: an actor-message string id must
        // index the packet's string table.
        let mk = |payload: Vec<Value>, string_table: Vec<String>| Packet::ActorMessage {
            target_actor: 1,
            behavior_name: "h".into(),
            content_hash: None,
            payload,
            string_table,
            sender_actor: 0,
            sender_node: NodeId(5),
            priority: MessagePriority::Normal,
        };
        assert!(packet_payload_wire_safe(&mk(vec![Value::int(1)], vec![])));
        assert!(packet_payload_wire_safe(&mk(
            vec![Value::string(0)],
            vec!["hello".into()]
        )));
        // Dangling id: no table entry at index 3.
        assert!(!packet_payload_wire_safe(&mk(vec![Value::string(3)], vec![])));
        // Heap values stay rejected even with a table present.
        assert!(!packet_payload_wire_safe(&mk(
            vec![Value::ptr(std::ptr::null_mut())],
            vec!["x".into()]
        )));

        // Spawn requests have no receiving-side pool, so strings stay
        let spawn = Packet::SpawnRequest {
            request_id: 1,
            behavior_name: "Counter".into(),
            content_hash: None,
            initial_state: vec![("name".into(), Value::string(1))],
            bytecode: None,
        };
        assert!(!packet_payload_wire_safe(&spawn));

        assert!(packet_payload_wire_safe(&Packet::Heartbeat {
            node_id: NodeId(1),
            timestamp: 0,
        }));
    }

    #[test]
    fn test_nil_wire_roundtrip() {
        // Nil must serialize and deserialize as nil (not as a float).
        let mut buf = Vec::new();
        write_value(&mut buf, &Value::nil());
        assert!(!buf.is_empty());
        let (val, consumed) = read_value(&buf, 0).expect("nil should deserialize");
        assert!(val.is_nil(), "deserialized value must be nil");
        assert_eq!(consumed, 1, "nil tag has no payload bytes");

        // Roundtrip: nil written then read should match.
        let mut buf2 = Vec::new();
        write_value(&mut buf2, &val);
        assert_eq!(buf, buf2, "nil roundtrip must be stable");
    }

    #[test]
    fn test_transport_send_rejects_dangling_string_payload() {
        let addr_a = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0);
        let mut transport_a = TcpTransport::bind(addr_a).unwrap();
        let addr_b = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0);
        let mut transport_b = TcpTransport::bind(addr_b).unwrap();

        let addr_b_actual = transport_b.listen_addr();
        let node_b_id = transport_b.node_id();

        transport_a.connect(node_b_id, addr_b_actual).unwrap();
        sleep(Duration::from_millis(100));

        // A string id without a string-table entry is a dangling reference
        // that would resolve to the wrong string (or nil) on the receiving
        // node. The transport must drop the packet at send time rather than
        // deliver corrupt data.
        let bad = Packet::ActorMessage {
            target_actor: 1,
            behavior_name: "handle".into(),
            content_hash: None,
            payload: vec![Value::string(42)],
            string_table: vec![],
            sender_actor: 7,
            sender_node: transport_a.node_id(),
            priority: MessagePriority::Normal,
        };
        transport_a.send(node_b_id, addr_b_actual, bad);

        // Give the (non-)delivery plenty of time, then confirm nothing came.
        sleep(Duration::from_millis(500));
        let received = transport_b.receive();
        assert!(
            received
                .iter()
                .all(|p| !matches!(p.packet, Packet::ActorMessage { .. })),
            "dangling string payload must be rejected at send time, but B received: {:?}",
            received
        );

        transport_a.shutdown();
        transport_b.shutdown();
    }

    #[test]
    fn test_transport_send_delivers_string_payload_with_table() {
        let addr_a = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0);
        let mut transport_a = TcpTransport::bind(addr_a).unwrap();
        let addr_b = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0);
        let mut transport_b = TcpTransport::bind(addr_b).unwrap();

        let addr_b_actual = transport_b.listen_addr();
        let node_b_id = transport_b.node_id();

        transport_a.connect(node_b_id, addr_b_actual).unwrap();
        sleep(Duration::from_millis(100));

        // A string payload whose content travels in the packet's string
        let good = Packet::ActorMessage {
            target_actor: 1,
            behavior_name: "handle".into(),
            content_hash: None,
            payload: vec![Value::string(0), Value::int(7), Value::string(1)],
            string_table: vec!["hello".into(), "world".into()],
            sender_actor: 7,
            sender_node: transport_a.node_id(),
            priority: MessagePriority::Normal,
        };
        transport_a.send(node_b_id, addr_b_actual, good.clone());

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut received = Vec::new();
        while Instant::now() < deadline && received.is_empty() {
            received = transport_b.receive();
            if received.is_empty() {
                sleep(Duration::from_millis(50));
            }
        }

        assert!(
            received.iter().any(|p| p.packet == good),
            "string payload with content table must be delivered, got: {:?}",
            received
        );

        transport_a.shutdown();
        transport_b.shutdown();
    }

    #[test]
    fn test_transport_send_delivers_scalar_payload() {
        let addr_a = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0);
        let mut transport_a = TcpTransport::bind(addr_a).unwrap();
        let addr_b = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0);
        let mut transport_b = TcpTransport::bind(addr_b).unwrap();

        let addr_b_actual = transport_b.listen_addr();
        let node_b_id = transport_b.node_id();

        transport_a.connect(node_b_id, addr_b_actual).unwrap();
        sleep(Duration::from_millis(100));

        // Scalar payloads are wire-safe and must be delivered unchanged —
        let good = Packet::ActorMessage {
            target_actor: 1,
            behavior_name: "handle".into(),
            content_hash: None,
            payload: vec![Value::int(123), Value::bool(true), Value::unit()],
            string_table: vec![],
            sender_actor: 7,
            sender_node: transport_a.node_id(),
            priority: MessagePriority::Normal,
        };
        transport_a.send(node_b_id, addr_b_actual, good.clone());

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut received = Vec::new();
        while Instant::now() < deadline && received.is_empty() {
            received = transport_b.receive();
            if received.is_empty() {
                sleep(Duration::from_millis(50));
            }
        }

        assert!(
            received.iter().any(|p| p.packet == good),
            "scalar payload must be delivered, got: {:?}",
            received
        );

        transport_a.shutdown();
        transport_b.shutdown();
    }

    // ------------------------------------------------------------------
    // 10. Sequence numbers increment
    // ------------------------------------------------------------------
    #[test]
    fn test_transport_sequence_numbers() {
        let addr_a = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0);
        let mut transport_a = TcpTransport::bind(addr_a).unwrap();

        let addr_b = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0);
        let mut transport_b = TcpTransport::bind(addr_b).unwrap();

        let addr_b_actual = transport_b.listen_addr();
        let node_b_id = transport_b.node_id();

        transport_a.connect(node_b_id, addr_b_actual).unwrap();
        sleep(Duration::from_millis(100));

        // Send several packets and capture their sequence numbers on the wire
        // by serialising them locally with the transport's counter.
        let seq1 = transport_a.next_seq.load(Ordering::SeqCst);
        transport_a.send(node_b_id, addr_b_actual, Packet::Ack { packet_seq: 1 });

        let seq2 = transport_a.next_seq.load(Ordering::SeqCst);
        transport_a.send(node_b_id, addr_b_actual, Packet::Ack { packet_seq: 2 });

        let seq3 = transport_a.next_seq.load(Ordering::SeqCst);
        transport_a.send(node_b_id, addr_b_actual, Packet::Ack { packet_seq: 3 });

        assert_eq!(seq2, seq1 + 1, "sequence numbers must be monotonic");
        assert_eq!(seq3, seq2 + 1, "sequence numbers must be monotonic");
        assert_eq!(seq3, seq1 + 2, "sequence numbers must increment by 1 each");

        transport_a.shutdown();
        transport_b.shutdown();
    }

    // ------------------------------------------------------------------
    // 11. SpawnRequest / SpawnResponse roundtrip
    // ------------------------------------------------------------------
    #[test]
    fn test_packet_spawn_roundtrip() {
        let req = Packet::SpawnRequest {
            request_id: 12345,
            behavior_name: "Counter".into(),
            content_hash: None,
            initial_state: vec![
                ("count".into(), Value::int(0)),
                ("name".into(), Value::string(42)),
            ],
            bytecode: None,
        };
        let bytes = req.to_bytes(99);
        let (seq, decoded) = Packet::from_bytes(&bytes).unwrap();
        assert_eq!(seq, 99);
        assert_eq!(decoded, req);

        let resp = Packet::SpawnResponse {
            request_id: 12345,
            actor_id: 999,
            success: true,
        };
        let bytes = resp.to_bytes(100);
        let (seq, decoded) = Packet::from_bytes(&bytes).unwrap();
        assert_eq!(seq, 100);
        assert_eq!(decoded, resp);
    }

    // ------------------------------------------------------------------
    // 12. Corrupt / garbage bytes are rejected
    // ------------------------------------------------------------------
    #[test]
    fn test_packet_from_bytes_rejects_garbage() {
        assert!(Packet::from_bytes(b"").is_none());
        assert!(Packet::from_bytes(b"NUL").is_none());
        assert!(Packet::from_bytes(b"XXXX\x00\x00\x00\x00\x00\x00\x00\x00\x00").is_none());
        assert!(Packet::from_bytes(b"NUL0\xFF\x00\x00\x00\x00\x00\x00\x00\x00").is_none());
    }

    // ------------------------------------------------------------------
    // 13. Ack roundtrip
    // ------------------------------------------------------------------
    #[test]
    fn test_packet_ack_roundtrip() {
        let packet = Packet::Ack {
            packet_seq: 0xCAFE_BABE,
        };
        let bytes = packet.to_bytes(42);
        let (seq, decoded) = Packet::from_bytes(&bytes).unwrap();
        assert_eq!(seq, 42);
        assert_eq!(decoded, packet);
    }

    // ------------------------------------------------------------------
    // 14. Disconnect removes connection
    // ------------------------------------------------------------------
    #[test]
    fn test_transport_disconnect() {
        let addr_a = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0);
        let mut transport_a = TcpTransport::bind(addr_a).unwrap();

        let addr_b = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0);
        let mut transport_b = TcpTransport::bind(addr_b).unwrap();

        let node_b_id = transport_b.node_id();

        transport_a
            .connect(node_b_id, transport_b.listen_addr())
            .unwrap();
        sleep(Duration::from_millis(100));

        assert!(transport_a.connection_count() >= 1);
        transport_a.disconnect(node_b_id);
        assert_eq!(transport_a.connection_count(), 0);

        transport_a.shutdown();
        transport_b.shutdown();
    }
}
impl NetworkTransport for TcpTransport {
    fn connect(&mut self, node_id: NodeId, addr: std::net::SocketAddr) -> std::io::Result<()> {
        self.connect(node_id, addr)
    }
    fn send(&mut self, to_node: NodeId, to_addr: std::net::SocketAddr, packet: Packet) {
        self.send(to_node, to_addr, packet)
    }
    fn receive(&self) -> Vec<IncomingPacket> {
        self.receive()
    }
    fn node_id(&self) -> NodeId {
        self.node_id()
    }
    fn listen_addr(&self) -> std::net::SocketAddr {
        self.listen_addr()
    }
    fn disconnect(&mut self, node_id: NodeId) {
        self.disconnect(node_id)
    }
    fn shutdown(&mut self) {
        self.shutdown()
    }
    fn connection_count(&self) -> usize {
        self.connection_count()
    }
    fn connection_addr(&self, node_id: NodeId) -> Option<std::net::SocketAddr> {
        self.connection_addr(node_id)
    }
}
