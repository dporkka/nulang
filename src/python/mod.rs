//! Python Interop for Nulang.
//!
//! Provides integration with the Python ecosystem via PyO3. All data
//! crossing the Rust/Python boundary is explicitly marshaled between
//! Nulang VM [`Value`](crate::vm::Value)s and Python objects.
//!
//! # Modules
//!
//! - **`bridge`** — Python interpreter bridge: GIL management, module
//!   imports, attribute access, function calls, and a global object
//!   registry for refcounted Python handles.
//! - **`marshal`** — Bidirectional conversion between Nulang VM
//!   [`Value`](crate::vm::Value)s and Python objects.
//!
//! # Usage
//!
//! ```nulang
//! let result = perform Python.call("torch", ["Tensor"], [[1.0, 2.0, 3.0]])
//! perform IO.print(result)
//! ```

pub mod bridge;
pub mod marshal;

pub use bridge::*;
pub use marshal::*;
