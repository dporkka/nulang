//! Python Interop for Nulang.
//!
//! Provides integration with the Python ecosystem via PyO3, using the
//! **Native Actor pattern** to isolate CPython's global mutable state
//! from the clean Rust runtime.
//!
//! # Architecture: Native Actor Pattern
//!
//! Python code runs **only** inside dedicated OS-thread actors. All data
//! crossing the Rust/Python boundary must be explicitly marshaled into
//! pure Nulang primitives (Int, Float, Bool, String, Array of
//! primitives). Complex Python objects never leak into the Nulang VM.
//!
//! This is analogous to Erlang's NIFs/ports or Go's cgo rules: the
//! foreign language is treated as an isolated hazard.
//!
//! ```text
//! Nulang Actor (green thread)          Native Actor (OS thread)
//! +------------------------+           +------------------------+
//! | pure Nulang values     |  marshal  | Python interpreter      |
//! | Int, Float, String     | <-------> | torch, transformers     |
//! | Array[Int], Array[F64] |           | numpy, etc.             |
//! +------------------------+           +------------------------+
//!          |                                      |
//!          v                                      v
//!    M:N scheduler                          dedicated GIL
//!    (work-stealing)                         (per native actor)
//! ```
//!
//! # Modules
//!
//! - **`bridge`** — Python interpreter bridge: GIL management, module
//!   imports, attribute access, function calls, and a global object
//!   registry for refcounted Python handles.
//! - **`marshal`** — Bidirectional conversion between Nulang VM
//!   [`Value`](crate::vm::Value)s and Python objects.
//! - **`native_actor`** — Native Actor pool: manages dedicated OS threads
//!   for Python execution, enforces marshal-only data crossing.
//!
//! # Usage
//!
//! ```nulang
//! -- Python interop via native actors
//! let result = perform Python.call("torch", ["Tensor"], [[1.0, 2.0, 3.0]])
//! perform IO.print(result)
//! ```

pub mod bridge;
pub mod marshal;
pub mod native_actor;

pub use bridge::*;
pub use marshal::*;
pub use native_actor::*;
