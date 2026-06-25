//! Actor runtime system for Nulang.
//!
//! Provides: actor lifecycle, scheduler, mailbox, heap, GC, supervision,
//! distribution.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

mod actor;
mod scheduler;
mod mailbox;
mod heap;
mod dual_heap;
mod gc;
mod orca_cycle;
mod supervisor;
mod cluster;
mod network;
mod distributed;
mod crdt;
mod crdt_reg;
mod crdt_manager;
mod timer;
mod registry;
mod process_groups;

#[cfg(test)]
mod tests;

pub use actor::*;
pub use scheduler::*;
pub use mailbox::*;
pub use heap::*;
pub use dual_heap::*;
pub use gc::*;
pub use supervisor::*;
pub use orca_cycle::*;
pub use cluster::*;
pub use distributed::*;
pub use network::*;
pub use crdt::*;
pub use crdt_reg::*;
pub use crdt_manager::*;
pub use timer::*;
pub use registry::*;
pub use process_groups::*;

use crate::types::ExitReason;
use crate::vm::Value;
