//! Python Interop for Nulang.
//!
//! Provides bidirectional integration with the Python ecosystem via PyO3.
//! Enables importing Python modules, calling Python functions, and
//! converting data between Nulang and Python values.
//!
//! # Architecture
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
//! import python "torch" as torch
//! let tensor = torch.Tensor([1.0, 2.0, 3.0])
//! let result = tensor.sum()
//! ```
//!
//! # Value Tagging
//!
//! Python object references use a dedicated NaN tag:
//!
//! ```ignore
//! const TAG_PYTHON: u64 = 0x7FFE000000000000;
//! ```
//!
//! A [`Value`](crate::vm::Value) with this tag stores a 48-bit index
//! into the global [`PYTHON_REGISTRY`](bridge::PYTHON_REGISTRY). The
//! registry holds `PyObject` handles (refcounted pointers to Python
//! objects). Cloning a `PyObject` merely increments Python's reference
//! count — it is cheap and safe.

pub mod bridge;
pub mod marshal;

pub use bridge::*;
pub use marshal::*;
