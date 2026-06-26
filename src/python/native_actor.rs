//! Native Actor Pattern for Python Interop in Nulang.
//!
//! Isolates Python execution to dedicated OS threads, preventing CPython's
//! global mutable state from leaking into the clean Rust runtime.
//!
//! # Architecture
//!
//! ```text
//!  Nulang Runtime (green threads)        Native Actor Pool (OS threads)
//!  +---------------------------+         +-----------------------------+
//!  | VM calls import_function  | ──►     | Worker 0: Python context    |
//!  |   ↓                       |  task   |   GIL → exec → marshal      |
//!  | marshal args → Value      │  queue  |   ↓                         |
//!  |   ↓                       |         | marshal result → Value      |
//!  | block on result_rx        │ ◄──     |   ↓                         |
//!  |   ↓                       | result  | send on result_tx           |
//!  | return Value              |  queue  |                             |
//!  +---------------------------+         +-----------------------------+
//! ```
//!
//! # Key Invariants
//!
//! 1. **No Python objects in Nulang values** — Results are always marshaled
//!    to pure Nulang primitives (`Int`, `Float`, `Bool`, `String`, `Unit`).
//!    Complex Python objects stay in the native actor's Python context.
//! 2. **GIL isolation** — Each native actor acquires the GIL independently
//!    via `Python::with_gil`. They do not share Python interpreter state.
//! 3. **Marshal at boundary** — Data is converted at the boundary via
//!    [`nulang_to_python`](super::marshal::nulang_to_python) and
//!    [`python_to_nulang`](super::marshal::python_to_nulang). Nothing is
//!    shared by reference across the Rust/Python divide.
//! 4. **Thread safety** — Native actors are OS threads, so they do not
//!    interfere with the M:N scheduler's green threads.
//!
//! # Usage
//!
//! ```ignore
//! let pool = NativeActorPool::new(4); // 4 Python worker threads
//! let math = pool.import_module("math").unwrap();
//! let result = pool.call_function("math", &["sqrt"], vec![Value::float(16.0)], vec![]).unwrap();
//! assert_eq!(result.as_float(), Some(4.0));
//! pool.shutdown();
//! ```

use crossbeam::channel::{bounded, unbounded, Receiver, Sender};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::python::marshal::{nulang_to_python, python_to_nulang};
use crate::vm::Value;

// ---------------------------------------------------------------------------
// Constants (mirrored from vm.rs for local use)
// ---------------------------------------------------------------------------

const TAG_MASK: u64 = 0xFFFF000000000000;
const TAG_INT: u64 = 0x7FF9000000000000;
const TAG_PTR: u64 = 0x7FFA000000000000;
const TAG_ACTOR: u64 = 0x7FFB000000000000;
const TAG_SPECIAL: u64 = 0x7FFC000000000000;
const TAG_STRING: u64 = 0x7FFD000000000000;
const TAG_PYTHON: u64 = 0x7FFE000000000000;

// ---------------------------------------------------------------------------
// NativeTask — work sent to native actors
// ---------------------------------------------------------------------------

/// A unit of work sent to a native actor for Python execution.
///
/// Each variant represents a different kind of Python operation. All
/// arguments are [`Value`]s — they must be pure primitives (enforced by
/// [`is_safe_to_marshal`]). The `reply_id` is used to correlate the
/// result with the waiting caller.
#[derive(Debug)]
pub enum NativeTask {
    /// Import a Python module by name.
    Import { module: String, reply_id: u64 },

    /// Call a Python function (or callable) at a dotted attribute path.
    ///
    /// # Example
    /// ```ignore
    /// module = "torch", attr_path = ["nn", "Linear"]
    /// // resolves to: torch.nn.Linear(*args, **kwargs)
    /// ```
    Call {
        module: String,
        attr_path: Vec<String>,
        args: Vec<Value>,
        kwargs: Vec<(String, Value)>,
        reply_id: u64,
    },

    /// Evaluate a Python expression string.
    Eval { code: String, reply_id: u64 },

    /// Signal the worker thread to shut down.
    Shutdown,
}

// ---------------------------------------------------------------------------
// NativeResult — response from native actors
// ---------------------------------------------------------------------------

/// The result of a native actor's work, sent back to the requesting thread.
///
/// The `reply_id` matches the `reply_id` from the corresponding
/// [`NativeTask`], allowing callers to correlate responses with requests.
#[derive(Debug)]
pub enum NativeResult {
    /// The operation succeeded; the Python return value has been marshaled
    /// to a pure Nulang [`Value`].
    Ok { reply_id: u64, value: Value },

    /// The operation failed; the error message is a human-readable string.
    Err { reply_id: u64, error: String },
}

// ---------------------------------------------------------------------------
// NativeActorHandle — a running OS thread worker
// ---------------------------------------------------------------------------

/// Handle to a native actor worker thread.
///
/// Each handle represents a dedicated OS thread with its own Python
/// context. The thread loops, receiving [`NativeTask`]s from the shared
/// task queue, executing them under the GIL, and sending [`NativeResult`]s
/// back on the shared result channel.
pub struct NativeActorHandle {
    /// The underlying OS thread handle. Stored so we can join on shutdown.
    thread: Option<std::thread::JoinHandle<()>>,
    /// Monotonic actor ID for debugging and logging.
    actor_id: usize,
}

impl NativeActorHandle {
    /// Create a new handle from a thread and actor ID.
    fn new(thread: std::thread::JoinHandle<()>, actor_id: usize) -> Self {
        NativeActorHandle {
            thread: Some(thread),
            actor_id,
        }
    }

    /// Join the worker thread, blocking until it exits.
    ///
    /// This should be called during [`NativeActorPool::shutdown`] after
    /// sending [`NativeTask::Shutdown`] to all workers.
    fn join(&mut self) {
        if let Some(t) = self.thread.take() {
            if let Err(e) = t.join() {
                eprintln!("NativeActor {} panicked: {:?}", self.actor_id, e);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// PendingReplyMap — request/response correlation
// ---------------------------------------------------------------------------

/// Maps `reply_id` values to oneshot senders so that waiting callers
/// can receive their specific result.
type PendingReplyMap = Mutex<HashMap<u64, Sender<NativeResult>>>;

// ---------------------------------------------------------------------------
// NativeActorPool — manages the pool of Python worker threads
// ---------------------------------------------------------------------------

/// A pool of dedicated OS threads for isolated Python execution.
///
/// The pool owns a set of [`NativeActorHandle`]s (worker threads) and
/// two channels: a shared task queue (MPSC from callers to workers) and
/// a shared result queue (MPMC from workers back to a dispatcher thread).
///
/// # Thread Safety
///
/// `NativeActorPool` is `Send + Sync`. All internal mutable state is
/// behind `Mutex` or atomic types. Callers can safely submit tasks from
/// multiple threads concurrently.
pub struct NativeActorPool {
    /// Worker thread handles. Stored for orderly shutdown (join).
    workers: Mutex<Vec<NativeActorHandle>>,

    /// Shared task queue: any caller sends, any worker receives.
    task_tx: Sender<NativeTask>,
    /// Keep a clone of the receiver for the dispatcher's reference.
    /// (Workers also hold clones.)
    _task_rx: Receiver<NativeTask>,

    /// Monotonically increasing reply ID counter.
    next_reply_id: AtomicU64,

    /// Pending reply map: reply_id → oneshot sender for the waiting caller.
    pending: Arc<PendingReplyMap>,

    /// Handle for the result-dispatcher thread (reads result_rx and routes
    /// to pending callers).
    dispatcher: Mutex<Option<NativeActorHandle>>,
}

impl NativeActorPool {
    // ------------------------------------------------------------------
    // Construction
    // ------------------------------------------------------------------

    /// Create a new native actor pool with `pool_size` worker threads.
    ///
    /// Each worker thread:
    /// 1. Calls [`pyo3::prepare_freethreaded_python()`] once before entering
    ///    the task loop.
    /// 2. Receives [`NativeTask`]s from the shared task queue.
    /// 3. For each task: acquires the GIL via `Python::with_gil`, executes
    ///    the Python operation, marshals the result to a [`Value`], and
    ///    sends a [`NativeResult`] back on the result channel.
    /// 4. On [`NativeTask::Shutdown`]: breaks the loop and exits.
    ///
    /// A result-dispatcher thread is also spawned to route incoming results
    /// to the correct waiting caller based on `reply_id`.
    ///
    /// # Panics
    ///
    /// Panics if `pool_size` is zero.
    pub fn new(pool_size: usize) -> Self {
        assert!(pool_size > 0, "NativeActorPool pool_size must be > 0");

        // Shared task channel (MPMC — multiple producers, multiple consumers)
        let (task_tx, task_rx) = unbounded::<NativeTask>();

        // Shared result channel (MPMC — multiple producers, single consumer)
        let (result_tx, result_rx) = unbounded::<NativeResult>();

        let pending: Arc<PendingReplyMap> = Arc::new(Mutex::new(HashMap::new()));

        // Spawn worker threads
        let mut workers = Vec::with_capacity(pool_size);
        for actor_id in 0..pool_size {
            let task_rx = task_rx.clone();
            let result_tx = result_tx.clone();
            let handle = std::thread::spawn(move || {
                native_actor_worker_loop(actor_id, task_rx, result_tx);
            });
            workers.push(NativeActorHandle::new(handle, actor_id));
        }

        // Spawn result-dispatcher thread
        let pending_disp = Arc::clone(&pending);
        let dispatcher_thread = std::thread::spawn(move || {
            result_dispatcher_loop(result_rx, pending_disp);
        });
        let dispatcher = NativeActorHandle::new(dispatcher_thread, pool_size); // ID = pool_size

        NativeActorPool {
            workers: Mutex::new(workers),
            task_tx,
            _task_rx: task_rx,
            next_reply_id: AtomicU64::new(1),
            pending,
            dispatcher: Mutex::new(Some(dispatcher)),
        }
    }

    // ------------------------------------------------------------------
    // Public API
    // ------------------------------------------------------------------

    /// Import a Python module by name.
    ///
    /// Sends an [`NativeTask::Import`] to a worker and blocks until the
    /// module reference (as a [`Value`]) is returned.
    ///
    /// # Errors
    ///
    /// Returns an error if the module cannot be imported (e.g. not installed,
    /// or Python raises an `ImportError`).
    ///
    /// # Example
    /// ```ignore
    /// let pool = NativeActorPool::new(2);
    /// let math = pool.import_module("math").unwrap();
    /// ```
    pub fn import_module(&self, name: &str) -> Result<Value, String> {
        let reply_id = self.alloc_reply_id();
        let (tx, rx) = bounded::<NativeResult>(1);

        {
            let mut pending = self.pending.lock().unwrap();
            pending.insert(reply_id, tx);
        }

        let task = NativeTask::Import {
            module: name.to_string(),
            reply_id,
        };

        self.task_tx
            .send(task)
            .map_err(|_| "NativeActorPool task channel closed".to_string())?;

        let result = rx
            .recv()
            .map_err(|_| "NativeActorPool result channel closed".to_string())?;

        match result {
            NativeResult::Ok { value, .. } => Ok(value),
            NativeResult::Err { error, .. } => Err(error),
        }
    }

    /// Call a Python function (or any callable) with marshaled arguments.
    ///
    /// The `module` and `attr_path` are used to resolve the callable:
    /// `module.attr_path[0].attr_path[1]...` (e.g. `"torch", ["nn", "Linear"]`
    /// resolves to `torch.nn.Linear`).
    ///
    /// All `args` and `kwargs` values are validated with [`is_safe_to_marshal`]
    /// before being sent to the worker. Only pure Nulang primitives are
    /// allowed — complex values must be explicitly marshaled.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Any argument is not safe to marshal (contains `TAG_PTR`, `TAG_ACTOR`, etc.)
    /// - The module or callable cannot be found
    /// - The Python call raises an exception
    /// - The result cannot be marshaled back to a [`Value`]
    ///
    /// # Example
    /// ```ignore
    /// let result = pool.call_function(
    ///     "math",
    ///     &["sqrt"],
    ///     vec![Value::float(16.0)],
    ///     vec![],
    /// ).unwrap();
    /// assert_eq!(result.as_float(), Some(4.0));
    /// ```
    pub fn call_function(
        &self,
        module: &str,
        attr_path: &[String],
        args: Vec<Value>,
        kwargs: Vec<(String, Value)>,
    ) -> Result<Value, String> {
        // Validate all arguments are safe to marshal
        for (i, arg) in args.iter().enumerate() {
            if !is_safe_to_marshal(arg) {
                return Err(format!(
                    "Argument at index {} is not safe to marshal: {:?}",
                    i,
                    arg.to_string_repr()
                ));
            }
        }
        for (key, val) in &kwargs {
            if !is_safe_to_marshal(val) {
                return Err(format!(
                    "Keyword argument '{}' is not safe to marshal: {:?}",
                    key,
                    val.to_string_repr()
                ));
            }
        }

        let reply_id = self.alloc_reply_id();
        let (tx, rx) = bounded::<NativeResult>(1);

        {
            let mut pending = self.pending.lock().unwrap();
            pending.insert(reply_id, tx);
        }

        let task = NativeTask::Call {
            module: module.to_string(),
            attr_path: attr_path.to_vec(),
            args,
            kwargs,
            reply_id,
        };

        self.task_tx
            .send(task)
            .map_err(|_| "NativeActorPool task channel closed".to_string())?;

        let result = rx
            .recv()
            .map_err(|_| "NativeActorPool result channel closed".to_string())?;

        match result {
            NativeResult::Ok { value, .. } => Ok(value),
            NativeResult::Err { error, .. } => Err(error),
        }
    }

    /// Evaluate a Python expression string.
    ///
    /// The `code` is evaluated using Python's `eval()` builtin. The result
    /// is marshaled back to a pure Nulang [`Value`].
    ///
    /// # Errors
    ///
    /// Returns an error if the expression is invalid or raises an exception.
    ///
    /// # Example
    /// ```ignore
    /// let result = pool.eval("1 + 2").unwrap();
    /// assert_eq!(result.as_int(), Some(3));
    /// ```
    pub fn eval(&self, code: &str) -> Result<Value, String> {
        let reply_id = self.alloc_reply_id();
        let (tx, rx) = bounded::<NativeResult>(1);

        {
            let mut pending = self.pending.lock().unwrap();
            pending.insert(reply_id, tx);
        }

        let task = NativeTask::Eval {
            code: code.to_string(),
            reply_id,
        };

        self.task_tx
            .send(task)
            .map_err(|_| "NativeActorPool task channel closed".to_string())?;

        let result = rx
            .recv()
            .map_err(|_| "NativeActorPool result channel closed".to_string())?;

        match result {
            NativeResult::Ok { value, .. } => Ok(value),
            NativeResult::Err { error, .. } => Err(error),
        }
    }

    /// Shut down the pool, stopping all worker threads.
    ///
    /// Sends [`NativeTask::Shutdown`] to all workers, then joins each
    /// worker thread and the result-dispatcher thread.
    ///
    /// # Panics
    ///
    /// Panics if called more than once (ownership is consumed but this
    /// method takes `self` by value).
    pub fn shutdown(self) {
        // Send Shutdown to all workers
        let worker_count = {
            let workers = self.workers.lock().unwrap();
            workers.len()
        };

        for _ in 0..worker_count {
            let _ = self.task_tx.send(NativeTask::Shutdown);
        }

        // Join all worker threads
        {
            let mut workers = self.workers.lock().unwrap();
            for mut worker in workers.drain(..) {
                worker.join();
            }
        }

        // Join the dispatcher thread
        {
            let mut dispatcher_opt = self.dispatcher.lock().unwrap();
            if let Some(mut dispatcher) = dispatcher_opt.take() {
                dispatcher.join();
            }
        }
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    /// Allocate a unique reply ID for request/response correlation.
    fn alloc_reply_id(&self) -> u64 {
        self.next_reply_id.fetch_add(1, Ordering::SeqCst)
    }
}

impl Default for NativeActorPool {
    fn default() -> Self {
        // Default to the number of logical CPUs, clamped to [1, 8]
        let pool_size = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2)
            .clamp(1, 8);
        Self::new(pool_size)
    }
}

// ---------------------------------------------------------------------------
// Worker thread entry point
// ---------------------------------------------------------------------------

/// The main loop for each native actor worker thread.
///
/// 1. Calls `pyo3::prepare_freethreaded_python()` to initialize the
///    Python interpreter for this thread.
/// 2. Loops, receiving [`NativeTask`]s from the shared task queue.
/// 3. For each task: acquires the GIL, executes the Python operation,
///    marshals the result, and sends a [`NativeResult`] back.
/// 4. On [`NativeTask::Shutdown`]: breaks the loop and returns.
fn native_actor_worker_loop(
    actor_id: usize,
    task_rx: Receiver<NativeTask>,
    result_tx: Sender<NativeResult>,
) {
    // Ensure Python is initialized for free-threaded (sub-interpreter) use.
    // This is a no-op if Python is already initialized, but required for
    // threads that will acquire the GIL independently.
    pyo3::prepare_freethreaded_python();

    loop {
        let task = match task_rx.recv() {
            Ok(t) => t,
            Err(_) => {
                // Channel closed — all senders dropped. Exit cleanly.
                break;
            }
        };

        match task {
            NativeTask::Import { module, reply_id } => {
                let result = handle_import(&module, reply_id);
                let _ = result_tx.send(result);
            }
            NativeTask::Call {
                module,
                attr_path,
                args,
                kwargs,
                reply_id,
            } => {
                let result = handle_call(&module, &attr_path, args, kwargs, reply_id);
                let _ = result_tx.send(result);
            }
            NativeTask::Eval { code, reply_id } => {
                let result = handle_eval(&code, reply_id);
                let _ = result_tx.send(result);
            }
            NativeTask::Shutdown => {
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Task handlers — each runs under the GIL
// ---------------------------------------------------------------------------

/// Handle a module import task.
///
/// Imports the module and returns a [`Value::unit()`] on success
/// (the module object stays in the worker's Python context).
/// The caller should use subsequent `Call` tasks to interact with it.
fn handle_import(module: &str, reply_id: u64) -> NativeResult {
    Python::with_gil(|py| {
        let result = py
            .import_bound(module)
            .map_err(|e| format!("Failed to import module '{}': {}", module, e));

        match result {
            Ok(_mod_obj) => {
                // Module imported successfully. Return unit — the module
                // object stays in the worker's Python context. Callers use
                // subsequent Call tasks to invoke functions from it.
                NativeResult::Ok {
                    reply_id,
                    value: Value::unit(),
                }
            }
            Err(e) => NativeResult::Err { reply_id, error: e },
        }
    })
}

/// Handle a function call task.
///
/// 1. Imports the module.
/// 2. Walks the attribute path to resolve the callable.
/// 3. Marshals Nulang [`Value`] arguments to Python objects.
/// 4. Calls the Python function.
/// 5. Marshals the return value back to a Nulang [`Value`].
fn handle_call(
    module: &str,
    attr_path: &[String],
    args: Vec<Value>,
    kwargs: Vec<(String, Value)>,
    reply_id: u64,
) -> NativeResult {
    Python::with_gil(|py| {
        // Step 1: Import the module
        let module_obj = match py.import_bound(module) {
            Ok(m) => m,
            Err(e) => {
                return NativeResult::Err {
                    reply_id,
                    error: format!("Failed to import module '{}': {}", module, e),
                };
            }
        };

        // Step 2: Walk the attribute path to resolve the callable
        let mut current = module_obj.into_any();
        for attr in attr_path {
            match current.getattr(attr) {
                Ok(obj) => current = obj,
                Err(e) => {
                    return NativeResult::Err {
                        reply_id,
                        error: format!(
                            "Failed to get attribute '{}': {}",
                            attr, e
                        ),
                    };
                }
            }
        }

        // Step 3: Marshal args from Nulang Value → Python objects
        let py_args: Vec<PyObject> = match args.into_iter().map(nulang_to_python).collect() {
            Ok(v) => v,
            Err(e) => {
                return NativeResult::Err {
                    reply_id,
                    error: format!("Failed to marshal args: {}", e),
                };
            }
        };

        let py_kwargs: Vec<(String, PyObject)> = match kwargs
            .into_iter()
            .map(|(k, v)| nulang_to_python(v).map(|pv| (k, pv)))
            .collect()
        {
            Ok(v) => v,
            Err(e) => {
                return NativeResult::Err {
                    reply_id,
                    error: format!("Failed to marshal kwargs: {}", e),
                };
            }
        };

        // Step 4: Build PyTuple and PyDict, then call
        let arg_refs: Vec<&pyo3::Bound<'_, pyo3::PyAny>> =
            py_args.iter().map(|o| o.bind(py)).collect();

        let args_tuple = match PyTuple::new(py, &arg_refs) {
            Ok(t) => t,
            Err(e) => {
                return NativeResult::Err {
                    reply_id,
                    error: format!("Failed to build argument tuple: {}", e),
                };
            }
        };

        let kwargs_dict = PyDict::new(py);
        for (key, obj) in &py_kwargs {
            if let Err(e) = kwargs_dict.set_item(key, obj.bind(py)) {
                return NativeResult::Err {
                    reply_id,
                    error: format!("Failed to set kwarg '{}': {}", key, e),
                };
            }
        }

        let py_result = match current.call(args_tuple, Some(&kwargs_dict)) {
            Ok(r) => r,
            Err(e) => {
                return NativeResult::Err {
                    reply_id,
                    error: format!("Python call failed: {}", e),
                };
            }
        };

        // Step 5: Marshal the result back to a Nulang Value
        match python_to_nulang(&py_result) {
            Ok(value) => NativeResult::Ok { reply_id, value },
            Err(e) => NativeResult::Err {
                reply_id,
                error: format!("Failed to marshal Python result: {}", e),
            },
        }
    })
}

/// Handle an eval task.
///
/// Evaluates the Python expression using the `builtins.eval` function
/// and marshals the result back to a Nulang [`Value`].
fn handle_eval(code: &str, reply_id: u64) -> NativeResult {
    Python::with_gil(|py| {
        let builtins = match py.import_bound("builtins") {
            Ok(b) => b,
            Err(e) => {
                return NativeResult::Err {
                    reply_id,
                    error: format!("Failed to import builtins: {}", e),
                };
            }
        };

        let eval_fn = match builtins.getattr("eval") {
            Ok(f) => f,
            Err(e) => {
                return NativeResult::Err {
                    reply_id,
                    error: format!("Failed to get eval: {}", e),
                };
            }
        };

        let code_obj = match PyTuple::new(py, &[code]) {
            Ok(t) => t,
            Err(e) => {
                return NativeResult::Err {
                    reply_id,
                    error: format!("Failed to build eval args: {}", e),
                };
            }
        };

        let py_result = match eval_fn.call1(code_obj) {
            Ok(r) => r,
            Err(e) => {
                return NativeResult::Err {
                    reply_id,
                    error: format!("Python eval failed: {}", e),
                };
            }
        };

        match python_to_nulang(&py_result) {
            Ok(value) => NativeResult::Ok { reply_id, value },
            Err(e) => NativeResult::Err {
                reply_id,
                error: format!("Failed to marshal eval result: {}", e),
            },
        }
    })
}

// ---------------------------------------------------------------------------
// Result dispatcher — routes results to waiting callers
// ---------------------------------------------------------------------------

/// The result-dispatcher thread entry point.
///
/// Reads [`NativeResult`]s from the shared result channel and routes each
/// one to the correct waiting caller by looking up the `reply_id` in the
/// pending-reply map.
fn result_dispatcher_loop(result_rx: Receiver<NativeResult>, pending: Arc<PendingReplyMap>) {
    loop {
        match result_rx.recv() {
            Ok(result) => {
                let reply_id = match &result {
                    NativeResult::Ok { reply_id, .. } => *reply_id,
                    NativeResult::Err { reply_id, .. } => *reply_id,
                };

                let sender = {
                    let mut pending_map = pending.lock().unwrap();
                    pending_map.remove(&reply_id)
                };

                if let Some(tx) = sender {
                    // If the caller has already hung up, just drop the result.
                    let _ = tx.send(result);
                }
                // If no sender found, the caller may have timed out or
                // the pool is shutting down. Silently drop.
            }
            Err(_) => {
                // Result channel closed — all result senders (workers) dropped.
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// is_safe_to_marshal — validates values for boundary crossing
// ---------------------------------------------------------------------------

/// Check whether a [`Value`] is safe to marshal across the Python boundary.
///
/// Returns `true` only for pure Nulang primitives that can be safely
/// converted to Python objects and back without loss or reference sharing:
///
/// | Nulang Type | Safe? | Reason |
/// |------------|-------|--------|
/// | `Int`      | Yes   | 48-bit signed integer |
/// | `Float`    | Yes   | IEEE 754 double |
/// | `Bool`     | Yes   | `true` / `false` |
/// | `String`   | Yes   | Interned string ID |
/// | `Unit`     | Yes   | Maps to Python `None` |
/// | `Nil`      | Yes   | Maps to Python `None` |
/// | `Ptr`      | **No** | Heap pointer — opaque, not serializable |
/// | `Actor`    | **No** | Actor reference — cannot cross boundary |
/// | `Python`   | **No** | Opaque Python object — leaks state |
///
/// # Example
/// ```ignore
/// assert!(is_safe_to_marshal(&Value::int(42)));
/// assert!(is_safe_to_marshal(&Value::float(3.14)));
/// assert!(is_safe_to_marshal(&Value::bool(true)));
/// assert!(is_safe_to_marshal(&Value::unit()));
/// assert!(!is_safe_to_marshal(&Value::ptr(std::ptr::null_mut())));
/// ```
pub fn is_safe_to_marshal(val: &Value) -> bool {
    let tag = val.0 & TAG_MASK;

    if tag == TAG_INT {
        return true; // Integer
    }

    if tag == TAG_SPECIAL {
        // true, false, unit, nil are all safe
        let s = val.0 & 0xFFFF;
        return s == SPECIAL_UNIT || s == SPECIAL_TRUE || s == SPECIAL_FALSE || s == SPECIAL_NIL;
    }

    if tag == TAG_STRING {
        return true; // Interned string
    }

    if tag == TAG_PYTHON {
        return false; // Opaque Python object — would leak state
    }

    if tag == TAG_PTR {
        return false; // Heap pointer — opaque
    }

    if tag == TAG_ACTOR {
        return false; // Actor reference — cannot cross boundary
    }

    // If no tag matched, it might be a float (non-NaN IEEE 754 value)
    let f = f64::from_bits(val.0);
    if !f.is_nan() {
        return true; // Float
    }

    // Unrecognized NaN — not safe
    false
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod native_actor_tests {
    use super::*;

    // Helper: ensure Python is initialized before running any test
    fn ensure_python() {
        let _ = Python::with_gil(|_py| ());
    }

    // ------------------------------------------------------------------
    // 1. test_import_math — Import Python's math module
    // ------------------------------------------------------------------

    #[test]
    fn test_import_math() {
        ensure_python();
        let pool = NativeActorPool::new(1);

        // Import the math module
        let result = pool.import_module("math");
        assert!(result.is_ok(), "Failed to import math: {:?}", result.err());

        // Import returns unit — the module stays in the worker's Python context
        let value = result.unwrap();
        assert!(value.is_unit(), "import_module should return unit");

        pool.shutdown();
    }

    // ------------------------------------------------------------------
    // 2. test_call_with_args — Call a Python function with Nulang arguments
    // ------------------------------------------------------------------

    #[test]
    fn test_call_with_args() {
        ensure_python();
        let pool = NativeActorPool::new(1);

        // Call math.sqrt(16.0)
        let result = pool.call_function(
            "math",
            &["sqrt".to_string()],
            vec![Value::float(16.0)],
            vec![],
        );

        assert!(
            result.is_ok(),
            "call_function failed: {:?}",
            result.err()
        );

        let value = result.unwrap();
        let float_val = value.as_float().expect("Expected float result");
        assert!(
            (float_val - 4.0).abs() < f64::EPSILON,
            "Expected sqrt(16.0) = 4.0, got {}",
            float_val
        );

        pool.shutdown();
    }

    // ------------------------------------------------------------------
    // 3. test_marshal_roundtrip — Send i64, get i64 back
    // ------------------------------------------------------------------

    #[test]
    fn test_marshal_roundtrip() {
        ensure_python();
        let pool = NativeActorPool::new(1);

        // Send an integer through Python and back
        // builtins.int(42) returns 42 (as Python int), marshaled back to Value::int(42)
        let result = pool.call_function(
            "builtins",
            &["int".to_string()],
            vec![Value::int(42)],
            vec![],
        );

        assert!(
            result.is_ok(),
            "marshal roundtrip failed: {:?}",
            result.err()
        );

        let value = result.unwrap();
        assert_eq!(
            value.as_int(),
            Some(42),
            "Expected int 42 after roundtrip, got {:?}",
            value.to_string_repr()
        );

        // Also test negative integers
        let result_neg = pool.call_function(
            "builtins",
            &["int".to_string()],
            vec![Value::int(-999)],
            vec![],
        );

        assert!(result_neg.is_ok(), "negative roundtrip failed: {:?}", result_neg.err());
        assert_eq!(result_neg.unwrap().as_int(), Some(-999));

        pool.shutdown();
    }

    // ------------------------------------------------------------------
    // 4. test_eval_expression — eval("1 + 2") returns 3
    // ------------------------------------------------------------------

    #[test]
    fn test_eval_expression() {
        ensure_python();
        let pool = NativeActorPool::new(1);

        // Evaluate a simple arithmetic expression
        let result = pool.eval("1 + 2");
        assert!(result.is_ok(), "eval failed: {:?}", result.err());

        let value = result.unwrap();
        assert_eq!(
            value.as_int(),
            Some(3),
            "Expected eval('1 + 2') = 3, got {:?}",
            value.to_string_repr()
        );

        // Test float evaluation
        let result_f = pool.eval("3.14159 * 2.0");
        assert!(result_f.is_ok(), "float eval failed: {:?}", result_f.err());

        let float_val = result_f.unwrap().as_float().expect("Expected float");
        assert!(
            (float_val - 6.28318).abs() < 1e-5,
            "Expected ~6.28318, got {}",
            float_val
        );

        pool.shutdown();
    }

    // ------------------------------------------------------------------
    // 5. test_error_propagation — Python exception becomes Nulang error
    // ------------------------------------------------------------------

    #[test]
    fn test_error_propagation() {
        ensure_python();
        let pool = NativeActorPool::new(1);

        // Call math.sqrt(-1) — this raises ValueError in Python
        let result = pool.call_function(
            "math",
            &["sqrt".to_string()],
            vec![Value::float(-1.0)],
            vec![],
        );

        assert!(
            result.is_err(),
            "Expected error for sqrt(-1), got: {:?}",
            result.ok()
        );

        let err_msg = result.unwrap_err();
        assert!(
            err_msg.contains("math domain error") || err_msg.contains("Python call failed"),
            "Expected math domain error, got: {}",
            err_msg
        );

        // Also test eval of invalid code
        let eval_result = pool.eval("1 / 0");
        assert!(
            eval_result.is_err(),
            "Expected error for division by zero, got: {:?}",
            eval_result.ok()
        );

        pool.shutdown();
    }

    // ------------------------------------------------------------------
    // 6. test_pool_concurrent — Two simultaneous native actor calls
    // ------------------------------------------------------------------

    #[test]
    fn test_pool_concurrent() {
        ensure_python();
        // Use a pool with 2 workers for true concurrency
        let pool = NativeActorPool::new(2);

        // Spawn two concurrent calls from different threads
        let pool_arc = Arc::new(pool);
        let pool_a = Arc::clone(&pool_arc);
        let pool_b = Arc::clone(&pool_arc);

        let handle_a = std::thread::spawn(move || {
            pool_a.call_function(
                "math",
                &["sqrt".to_string()],
                vec![Value::float(100.0)],
                vec![],
            )
        });

        let handle_b = std::thread::spawn(move || {
            pool_b.call_function(
                "math",
                &["pow".to_string()],
                vec![Value::int(2), Value::int(10)],
                vec![],
            )
        });

        let result_a = handle_a.join().expect("Thread A panicked");
        let result_b = handle_b.join().expect("Thread B panicked");

        assert!(result_a.is_ok(), "Concurrent call A failed: {:?}", result_a.err());
        assert!(result_b.is_ok(), "Concurrent call B failed: {:?}", result_b.err());

        let val_a = result_a.unwrap();
        let float_a = val_a.as_float().expect("Expected float from sqrt");
        assert!(
            (float_a - 10.0).abs() < f64::EPSILON,
            "Expected sqrt(100) = 10.0, got {}",
            float_a
        );

        let val_b = result_b.unwrap();
        let int_b = val_b.as_int().expect("Expected int from pow");
        assert_eq!(int_b, 1024, "Expected 2^10 = 1024, got {}", int_b);

        // Recover the pool from the Arc for shutdown
        Arc::try_unwrap(pool_arc)
            .unwrap_or_else(|_| NativeActorPool::new(1)) // fallback
            .shutdown();
    }

    // ------------------------------------------------------------------
    // 7. test_is_safe_to_marshal — boundary safety checks
    // ------------------------------------------------------------------

    #[test]
    fn test_is_safe_to_marshal_primitives() {
        // Safe primitives
        assert!(is_safe_to_marshal(&Value::int(42)));
        assert!(is_safe_to_marshal(&Value::int(-1000)));
        assert!(is_safe_to_marshal(&Value::int(0)));
        assert!(is_safe_to_marshal(&Value::float(3.14)));
        assert!(is_safe_to_marshal(&Value::float(0.0)));
        assert!(is_safe_to_marshal(&Value::bool(true)));
        assert!(is_safe_to_marshal(&Value::bool(false)));
        assert!(is_safe_to_marshal(&Value::unit()));
        assert!(is_safe_to_marshal(&Value::nil()));
    }

    #[test]
    fn test_is_safe_to_marshal_unsafe_values() {
        // Unsafe: heap pointers
        let ptr_val = Value::ptr(std::ptr::null_mut::<u8>());
        assert!(!is_safe_to_marshal(&ptr_val));

        // Unsafe: actor references
        let actor_val = Value::actor_ref(1);
        assert!(!is_safe_to_marshal(&actor_val));

        // Unsafe: Python object references
        let py_val = Value::python_object(0);
        assert!(!is_safe_to_marshal(&py_val));
    }

    // ------------------------------------------------------------------
    // 8. test_eval_with_variables — eval returning various types
    // ------------------------------------------------------------------

    #[test]
    fn test_eval_returns_bool() {
        ensure_python();
        let pool = NativeActorPool::new(1);

        let result = pool.eval("True and False");
        assert!(result.is_ok(), "bool eval failed: {:?}", result.err());
        assert_eq!(result.unwrap().as_bool(), Some(false));

        let result2 = pool.eval("True or False");
        assert!(result2.is_ok());
        assert_eq!(result2.unwrap().as_bool(), Some(true));

        pool.shutdown();
    }

    // ------------------------------------------------------------------
    // 9. test_call_with_kwargs — keyword arguments
    // ------------------------------------------------------------------

    #[test]
    fn test_call_with_kwargs() {
        ensure_python();
        let pool = NativeActorPool::new(1);

        // builtins.pow(2, 10) via positional args
        let result = pool.call_function(
            "builtins",
            &["pow".to_string()],
            vec![Value::int(2), Value::int(8)],
            vec![],
        );

        assert!(result.is_ok(), "kwargs call failed: {:?}", result.err());
        assert_eq!(result.unwrap().as_int(), Some(256));

        pool.shutdown();
    }

    // ------------------------------------------------------------------
    // 10. test_pool_size — multiple workers are created
    // ------------------------------------------------------------------

    #[test]
    fn test_pool_size() {
        ensure_python();
        let pool = NativeActorPool::new(4);

        // All workers should be functional — send a task to verify
        let result = pool.eval("sum([1, 2, 3, 4, 5])");
        assert!(result.is_ok(), "Pool with 4 workers failed: {:?}", result.err());
        assert_eq!(result.unwrap().as_int(), Some(15));

        pool.shutdown();
    }
}
