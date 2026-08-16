//! Callbacks bridging the bytecode VM to the shared actor runtime.
//!
//! When `run_source_shared` compiles actors into a bytecode module and drives
//! them through the VM loop, actor operations (`spawn`, `send`, state access)
//! must reach the same `Runtime` state used by the interpreter path.  The VM
//! is deliberately decoupled from the runtime, so this module adapts the two
//! worlds: [`RuntimeVmCallbacks`] holds a transient raw pointer to the shared
//! runtime and implements [`crate::vm::ActorVmCallbacks`] on top of it.

use super::network::{send_distributed, spawn_on_node, ActorAddress, NodeId};
use super::runtime::Runtime;
use std::sync::Arc;

#[cfg(feature = "ai-runtime")]
use super::agent;
#[cfg(feature = "http")]
use super::http::HttpServerState;

/// Raw-pointer implementation of `ActorVmCallbacks` that delegates to the
/// shared [`Runtime`].
///
/// A fresh callback value is installed immediately before each VM step
/// that may need it, and removed right after.  The pointer is therefore
/// never held across runtime re-entrancy boundaries.
pub(crate) struct RuntimeVmCallbacks {
    runtime: *mut Runtime,
    actor_id: u64,
}

impl RuntimeVmCallbacks {
    pub(crate) fn new(runtime: *mut Runtime, actor_id: u64) -> Self {
        Self { runtime, actor_id }
    }
}

// SAFETY: the callback is created fresh on the scheduler thread, installed
// on the VM for the duration of one behavior invocation, and dropped before
// the runtime is touched again.  The runtime never moves (it is owned by a
// Box or a stack frame that outlives the callback).
unsafe impl Send for RuntimeVmCallbacks {}

impl crate::vm::ActorVmCallbacks for RuntimeVmCallbacks {
    fn alloc(&mut self, size: usize, type_tag: crate::runtime::heap::TypeTag) -> Option<*mut u8> {
        unsafe {
            let rt = &mut *self.runtime;
            let actor = rt.actors.get_mut(&self.actor_id)?;
            actor.heap.alloc(size, type_tag)
        }
    }

    fn retain_ref(&mut self, ptr: *mut u8) {
        if ptr.is_null() {
            return;
        }
        unsafe {
            let header = &mut *crate::runtime::heap::ActorHeap::header_of(ptr);
            header.ref_count = header.ref_count.saturating_add(1);
        }
    }

    fn drop_ref(&mut self, ptr: *mut u8) {
        if ptr.is_null() {
            return;
        }
        unsafe {
            let rt = &mut *self.runtime;
            // Check if the pointer belongs to this actor's heap (within the
            // bump region). If not, it could be an LOS block — check the
            // size_class. For now, conservatively assume all pointers from
            // this callback belong to the actor's heap.
            if let Some(actor) = rt.actors.get_mut(&self.actor_id) {
                let header_ptr = crate::runtime::heap::ActorHeap::header_of(ptr);
                let header = &mut *header_ptr;
                if header.ref_count > 0 {
                    header.ref_count -= 1;
                }
                if header.ref_count == 0 && !header.sticky {
                    actor.heap.free(ptr);
                }
            }
        }
    }

    fn array_len(&mut self, ptr: *mut u8) -> Option<usize> {
        if ptr.is_null() {
            return None;
        }
        unsafe {
            let header = &*crate::runtime::heap::ActorHeap::header_of(ptr);
            if header.type_tag == crate::runtime::heap::TypeTag::Array {
                let payload_size = header
                    .size
                    .saturating_sub(crate::runtime::heap::ActorHeap::HEADER_SIZE);
                Some(payload_size / std::mem::size_of::<crate::vm::Value>())
            } else {
                None
            }
        }
    }

    fn spawn_actor(
        &mut self,
        module: &crate::bytecode::CodeModule,
        behavior_idx: usize,
        init: Vec<(String, crate::vm::Value)>,
    ) -> crate::vm::Value {
        // SAFETY: the callback is installed on the shared runtime VM only
        // while the runtime drives a behavior on the single scheduler
        // thread, so `runtime` is a live, exclusively-borrowed pointer.
        // Spawning mutates runtime state but never re-enters the VM.
        unsafe { (*self.runtime).spawn_from_module(module, behavior_idx, init) }
    }

    fn send_message(
        &mut self,
        target: crate::vm::Value,
        behavior_id: u16,
        args: &[crate::vm::Value],
    ) {
        if let Some(target_id) = target.as_actor_id() {
            // SAFETY: as above. `send_message_by_id` is safe mid-behavior:
            // it pushes mail, bumps ORCA foreign counts, and enqueues the
            // target; the receive-wait wake is deferred while the shared
            // VM is executing (see `Runtime::pending_receive_wakes`).
            unsafe { (*self.runtime).send_message_by_id(target_id, behavior_id, args) }
        }
    }

    fn ask_actor(
        &mut self,
        actor: crate::vm::Value,
        behavior_id: u16,
        args: &[crate::vm::Value],
    ) -> crate::vm::Value {
        if let Some(target_id) = actor.as_actor_id() {
            unsafe { (*self.runtime).ask_actor_by_id(target_id, behavior_id, args) }
        } else {
            crate::vm::Value::nil()
        }
    }

    fn current_actor_id(&self) -> Option<u64> {
        Some(self.actor_id)
    }

    fn get_state_field(&self, field: &str) -> crate::vm::Value {
        unsafe {
            if let Some(actor) = (*self.runtime).actors.get(&self.actor_id) {
                return actor
                    .get_state_field(field)
                    .unwrap_or(crate::vm::Value::nil());
            }
        }
        crate::vm::Value::nil()
    }

    fn set_state_field(&mut self, field: &str, value: crate::vm::Value) {
        unsafe {
            if let Some(actor) = (*self.runtime).actors.get_mut(&self.actor_id) {
                actor.set_state_field(field, value);
            }
        }
    }

    fn emit_event(&mut self, event: &str, args: &[crate::vm::Value]) {
        unsafe {
            (*self.runtime).emit_event(self.actor_id, event, args);
        }
    }

    fn wait_signal(&mut self, name: &str) -> crate::vm::SignalWaitResult {
        unsafe {
            if let Some(actor) = (*self.runtime).actors.get(&self.actor_id) {
                if actor.received_signals.iter().any(|(n, _)| n == name) {
                    return crate::vm::SignalWaitResult::Ready(crate::vm::Value::unit());
                }
            }
            crate::vm::SignalWaitResult::NotReady
        }
    }

    fn suspend_for_signal(&mut self, _name: &str, _vm_state: Option<crate::vm::SuspendedVmState>) {
        // State capture is handled by run_bytecode_at_offset after run_from
        // returns, avoiding aliasing the Runtime through this raw-pointer
        // callback while the VM borrow is active.
    }

    fn perform_effect(
        &mut self,
        effect_name: &str,
        regs: &[crate::vm::Value],
    ) -> Option<crate::vm::Value> {
        unsafe {
            if effect_name != "Timer" {
                return None;
            }
            let actor = (*self.runtime).actors.get(&self.actor_id)?;
            if !actor.is_workflow {
                return Some(crate::vm::Value::unit());
            }
            let vm = (*self.runtime).vm.as_mut()?;
            let module_idx = vm.current_module_idx()?;
            let string_id = regs.get(0)?.as_string_id()?;
            let name = vm.constant_string(module_idx, string_id)?;
            let duration_ms = regs.get(1)?.as_int()? as u64;
            (*self.runtime).schedule_workflow_timer(self.actor_id, &name, duration_ms);
            Some(crate::vm::Value::unit())
        }
    }

    fn perform_builtin_effect(
        &mut self,
        effect_name: &str,
        op_name: Option<&str>,
        constants: &[crate::bytecode::Constant],
        regs: &[crate::vm::Value],
    ) -> Option<crate::vm::Value> {
        unsafe {
            if effect_name == "Workflow" && op_name == Some("query") {
                let workflow_id = regs.get(0)?.as_actor_id()?;
                let string_id = regs.get(1)?.as_string_id()?;
                let query_name = match constants.get(string_id as usize) {
                    Some(crate::bytecode::Constant::String(s)) => s.clone(),
                    _ => return None,
                };
                return (*self.runtime).query_workflow(workflow_id, &query_name);
            }
            if effect_name == "Actor" {
                return (*self.runtime).perform_actor_builtin(
                    Some(self.actor_id),
                    op_name,
                    constants,
                    regs,
                );
            }

            if effect_name == "Crdt" {
                return (*self.runtime).perform_crdt_builtin(
                    Some(self.actor_id),
                    op_name,
                    constants,
                    regs,
                );
            }

            if effect_name == "Int" && op_name == Some("to_float") {
                let n = regs.first().and_then(|v| v.as_int()).unwrap_or(0);
                return Some(crate::vm::Value::float(n as f64));
            }
            if effect_name == "Float" && op_name == Some("to_int") {
                let x = regs.first().and_then(|v| v.as_float()).unwrap_or(0.0);
                return Some(crate::vm::Value::int(x as i64));
            }
            if effect_name == "Float" && op_name == Some("to_string") {
                let x = regs.first().and_then(|v| v.as_float()).unwrap_or(0.0);
                let s = format!("{}", x);
                if let Some(vm) = &mut (*self.runtime).vm {
                    return Some(vm.allocate_string(&s));
                }
                return Some(crate::vm::Value::nil());
            }
            if effect_name == "String" && op_name == Some("to_int") {
                let s = crate::vm::resolve_value_string(
                    constants,
                    *regs.first().unwrap_or(&crate::vm::Value::nil()),
                );
                let n: i64 = s.parse().unwrap_or(0);
                return Some(crate::vm::Value::int(n));
            }
            if effect_name == "String" && op_name == Some("to_float") {
                let s = crate::vm::resolve_value_string(
                    constants,
                    *regs.first().unwrap_or(&crate::vm::Value::nil()),
                );
                let f: f64 = s.parse().unwrap_or(0.0);
                return Some(crate::vm::Value::float(f));
            }
            if effect_name == "Timer" && op_name == Some("after") {
                let ms = regs.first().and_then(|v| v.as_int()).unwrap_or(0);
                if ms > 0 {
                    let callback_id = regs.get(1).and_then(|v| v.as_string_id());
                    let callback_name = callback_id.and_then(|id| {
                        constants.get(id as usize).and_then(|c| match c {
                            crate::bytecode::Constant::String(s) => Some(s.clone()),
                            _ => None,
                        })
                    });
                    if let Some(callback_name) = callback_name {
                        let behavior_id = (*self.runtime)
                            .behavior_id_for(self.actor_id, &callback_name)
                            .unwrap_or(0);
                        if behavior_id > 0 {
                            (*self.runtime).timer_wheel.send_after(
                                std::time::Duration::from_millis(ms as u64),
                                self.actor_id,
                                behavior_id,
                                vec![],
                            );
                        }
                    }
                }
                return Some(crate::vm::Value::unit());
            }
            if effect_name == "Int" && op_name == Some("to_string") {
                let n = regs.first().and_then(|v| v.as_int()).unwrap_or(0);
                let s = format!("{}", n);
                if let Some(vm) = &mut (*self.runtime).vm {
                    return Some(vm.allocate_string(&s));
                }
                return Some(crate::vm::Value::nil());
            }

            if effect_name == "String" && op_name == Some("length") {
                let s = crate::vm::resolve_value_string(
                    constants,
                    *regs.first().unwrap_or(&crate::vm::Value::nil()),
                );
                return Some(crate::vm::Value::int(s.len() as i64));
            }
            if effect_name == "String" && op_name == Some("charAt") {
                let s = crate::vm::resolve_value_string(
                    constants,
                    *regs.first().unwrap_or(&crate::vm::Value::nil()),
                );
                let idx = regs.get(1).and_then(|v| v.as_int()).unwrap_or(-1);
                if idx < 0 || idx as usize >= s.len() {
                    return Some(crate::vm::Value::int(-1));
                }
                return Some(crate::vm::Value::int(s.as_bytes()[idx as usize] as i64));
            }
            if effect_name == "Provider" && op_name == Some("ask") {
                // General runtime-registered provider dispatch (actor path).
                let provider = match regs.get(0).and_then(|v| v.as_string_id()) {
                    Some(id) => match constants.get(id as usize) {
                        Some(crate::bytecode::Constant::String(s)) => s.clone(),
                        _ => return None,
                    },
                    None => return None,
                };
                let prompt = match regs.get(1) {
                    Some(v) => {
                        if let Some(id) = v.as_string_id() {
                            constants
                                .get(id as usize)
                                .and_then(|c| match c {
                                    crate::bytecode::Constant::String(s) => Some(s.clone()),
                                    _ => None,
                                })
                                .unwrap_or_default()
                        } else {
                            v.to_string_repr()
                        }
                    }
                    None => return None,
                };
                return (*self.runtime).perform_provider_ask(&provider, &prompt);
            }
            if effect_name == "Debug" && op_name == Some("inspect") {
                let target_id = regs.first().and_then(|v| v.as_int()).unwrap_or(0) as u64;
                let rt = &mut *self.runtime;
                let info = serde_json::json!({
                    "state": rt.actors.get(&target_id).map(|a| {
                        a.state_data.iter().map(|(k, v)| {
                            (k.clone(), crate::vm::resolve_value_string(constants, *v))
                        }).collect::<std::collections::HashMap<_, _>>()
                    }).unwrap_or_default(),
                    "mailbox_size": rt.actors.get(&target_id).map(|a| a.mailbox.len()).unwrap_or(0),
                    "behaviors": rt.actors.get(&target_id).map(|a| {
                        a.behavior_table.iter().map(|b| b.name.clone()).collect::<Vec<_>>()
                    }).unwrap_or_default(),
                    "supervisor": rt.supervisors.get(&target_id).map(|_s| target_id),
                });
                let json = serde_json::to_string(&info).unwrap_or_default();
                if let Some(vm) = &mut rt.vm {
                    return Some(vm.allocate_string(&json));
                }
                return Some(crate::vm::Value::nil());
            }
            if effect_name == "IO" {
                if let (Some("print") | Some("println"), Some(first)) = (op_name, regs.first()) {
                    let msg = crate::vm::resolve_value_string(constants, *first);
                    println!("{}", msg);
                    return Some(crate::vm::Value::unit());
                }
            }
            self.perform_effect(effect_name, regs)
        }
    }

    fn perform_builtin_effect_in_module(
        &mut self,
        effect_name: &str,
        op_name: Option<&str>,
        module: &crate::bytecode::CodeModule,
        regs: &[crate::vm::Value],
    ) -> Option<crate::vm::Value> {
        let qualified = match op_name {
            Some(op) => format!("{}.{}", effect_name, op),
            None => effect_name.to_string(),
        };
        unsafe {
            // Check test handlers before real dispatch.
            if let Some(result) = (*self.runtime).check_test_handler(&qualified, regs) {
                return Some(result);
            }
            if effect_name == "Otp" {
                return (*self.runtime).perform_otp_builtin(op_name, module, regs);
            }
            #[cfg(feature = "http")]
            if effect_name == "Http" && op_name == Some("serve") {
                let port = regs.first().and_then(|v| v.as_int()).unwrap_or(0) as u16;
                let func_idx = match regs.get(1) {
                    Some(v) if v.is_closure() => {
                        let payload = v.as_raw() & crate::value_layout::PAYLOAD_MASK;
                        if payload & crate::vm::CLOSURE_ENV_FLAG != 0 {
                            return Some(crate::vm::Value::nil());
                        }
                        payload as usize
                    }
                    Some(v) => v.as_int().unwrap_or(0) as usize,
                    None => return Some(crate::vm::Value::nil()),
                };
                return match HttpServerState::bind(port, module.clone(), func_idx) {
                    Ok(server) => {
                        let actual_port = server.port;
                        (*self.runtime).http_server = Some(server);
                        Some(crate::vm::Value::int(actual_port as i64))
                    }
                    Err(_) => Some(crate::vm::Value::nil()),
                };
            }
            self.perform_builtin_effect(effect_name, op_name, &module.constants, regs)
        }
    }

    #[cfg(feature = "ai-runtime")]
    fn perform_async(
        &mut self,
        effect_op: &str,
        constants: &[crate::bytecode::Constant],
        args: &[crate::vm::Value],
    ) -> crate::vm::PerformAsyncResult {
        use crate::vm::PerformAsyncResult;
        match effect_op {
            "Timer.sleep" => {
                let ms = args.first().and_then(|v| v.as_int()).unwrap_or(0) as u64;
                unsafe {
                    let rt = &mut *self.runtime;
                    if let Some(actor) = rt.actors.get_mut(&self.actor_id) {
                        if actor.timer_sleep_fired {
                            actor.timer_sleep_fired = false;
                            return PerformAsyncResult::Ready(None);
                        }
                    }
                    if ms == 0 {
                        return PerformAsyncResult::Ready(None);
                    }
                    rt.timer_wheel
                        .timer_sleep_wake(std::time::Duration::from_millis(ms), self.actor_id);
                }
                PerformAsyncResult::Pending
            }
            _ => PerformAsyncResult::Ready(None),
        }
    }

    fn try_receive(&mut self) -> Option<(u16, crate::vm::Value)> {
        unsafe {
            let msg = {
                let actor = (*self.runtime).actors.get_mut(&self.actor_id)?;
                actor.mailbox.pop()?
            };
            // ORCA receiver protocol: hold heap pointers carried by the message.
            (*self.runtime).hold_payload_refs(self.actor_id, &*msg.payload);
            let val = msg
                .payload
                .first()
                .cloned()
                .unwrap_or(crate::vm::Value::unit());
            Some((msg.behavior_id, val))
        }
    }

    fn try_receive_match(
        &mut self,
        behavior_ids: &[u16],
    ) -> Option<(usize, Vec<crate::vm::Value>)> {
        unsafe {
            let (pos, payload) = {
                let actor = (*self.runtime).actors.get_mut(&self.actor_id)?;
                actor.mailbox.receive_match(behavior_ids)?
            };
            // ORCA receiver protocol: hold heap pointers carried by the message.
            (*self.runtime).hold_payload_refs(self.actor_id, &*payload);
            Some((
                pos,
                Arc::try_unwrap(payload).unwrap_or_else(|arc| (*arc).clone()),
            ))
        }
    }

    fn receive_wait_suspend(&mut self, timeout_ms: i64) -> bool {
        unsafe {
            let rt = &mut *self.runtime;
            let Some(actor) = rt.actors.get_mut(&self.actor_id) else {
                return false;
            };
            // A fired timeout resolves the wait exactly once: consume the
            // marker so the re-executed ReceiveWait writes the no-match
            // sentinel and a later wait starts clean.
            if actor.receive_wait.map(|w| w.timed_out).unwrap_or(false) {
                actor.receive_wait = None;
                return false;
            }
            // Non-positive timeouts poll once (Erlang-style non-blocking
            // receive). Synchronous entry points (ask_actor_sync: pipelines,
            // supervisors, debates, `Ask`) never suspend — same gating as
            // the non-blocking LLM path.
            if timeout_ms <= 0 || !rt.suspend_enabled {
                return false;
            }
            true
        }
    }

    fn receive_wait_matched(&mut self) {
        unsafe {
            let rt = &mut *self.runtime;
            let wait = rt
                .actors
                .get_mut(&self.actor_id)
                .and_then(|a| a.receive_wait.take());
            // A match resolves the wait: cancel the pending timeout so it
            // cannot fire into a later wait on this actor.
            if let Some(wait) = wait {
                rt.timer_wheel.cancel(wait.timer_id);
            }
        }
    }

    fn commit_receive_match(&mut self) {
        unsafe {
            if let Some(actor) = (*self.runtime).actors.get_mut(&self.actor_id) {
                actor.mailbox.commit_receive_match();
            }
        }
    }

    fn reset_receive_match(&mut self) {
        unsafe {
            if let Some(actor) = (*self.runtime).actors.get_mut(&self.actor_id) {
                actor.mailbox.reset_receive_match();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Bytecode-driven runtime callbacks for external library actors
// ---------------------------------------------------------------------------

/// Raw-pointer callbacks for `Runtime::execute_bytecode_actor` (external
/// library actors).  Mirrors [`RuntimeVmCallbacks`] but with a coarser
/// contract: the runtime pointer is a transient borrow while the runtime
/// drives a single behavior.
#[derive(Debug)]
pub(crate) struct BytecodeRuntimeCallbacks {
    pub(crate) runtime: *mut Runtime,
    pub(crate) actor_id: u64,
}

impl BytecodeRuntimeCallbacks {
    pub(crate) fn new(runtime: *mut Runtime, actor_id: u64) -> Self {
        Self { runtime, actor_id }
    }
}

// SAFETY: `runtime` is a transient borrow of the executing `Runtime` that
// is valid for the duration of the behavior invocation. The scheduler
// guarantees that a `Runtime` (and thus each callback instance wrapping a
// pointer to it) is only driven from one thread at a time, so no two
// threads can alias the `&mut Runtime` produced by dereferencing `runtime`.
unsafe impl Send for BytecodeRuntimeCallbacks {}
// SAFETY: shared references only grant access through `Sync` if methods can
// be called concurrently; all callback methods mutate through the raw
// pointer and are only invoked while the owning thread is executing the
// behavior, so cross-thread concurrent use cannot occur by construction.
unsafe impl Sync for BytecodeRuntimeCallbacks {}

impl crate::vm::ActorVmCallbacks for BytecodeRuntimeCallbacks {
    fn alloc(&mut self, size: usize, type_tag: crate::runtime::heap::TypeTag) -> Option<*mut u8> {
        unsafe {
            let rt = &mut *self.runtime;
            let actor = rt.actors.get_mut(&self.actor_id)?;
            actor.heap.alloc(size, type_tag)
        }
    }

    fn retain_ref(&mut self, ptr: *mut u8) {
        if ptr.is_null() {
            return;
        }
        unsafe {
            let header = &mut *crate::runtime::heap::ActorHeap::header_of(ptr);
            header.ref_count = header.ref_count.saturating_add(1);
        }
    }

    fn drop_ref(&mut self, ptr: *mut u8) {
        if ptr.is_null() {
            return;
        }
        unsafe {
            let rt = &mut *self.runtime;
            if let Some(actor) = rt.actors.get_mut(&self.actor_id) {
                let header_ptr = crate::runtime::heap::ActorHeap::header_of(ptr);
                let header = &mut *header_ptr;
                if header.ref_count > 0 {
                    header.ref_count -= 1;
                }
                if header.ref_count == 0 && !header.sticky {
                    actor.heap.free(ptr);
                }
            }
        }
    }

    fn array_len(&mut self, ptr: *mut u8) -> Option<usize> {
        if ptr.is_null() {
            return None;
        }
        unsafe {
            let header = &*crate::runtime::heap::ActorHeap::header_of(ptr);
            if header.type_tag == crate::runtime::heap::TypeTag::Array {
                let payload_size = header
                    .size
                    .saturating_sub(crate::runtime::heap::ActorHeap::HEADER_SIZE);
                Some(payload_size / std::mem::size_of::<crate::vm::Value>())
            } else {
                None
            }
        }
    }

    fn spawn_actor(
        &mut self,
        module: &crate::bytecode::CodeModule,
        behavior_idx: usize,
        init: Vec<(String, crate::vm::Value)>,
    ) -> crate::vm::Value {
        // SAFETY: the callback is installed on the shared runtime VM only
        // while the runtime drives a behavior on the single scheduler
        // thread, so `runtime` is a live, exclusively-borrowed pointer.
        // Spawning mutates runtime state but never re-enters the VM.
        unsafe { (*self.runtime).spawn_from_module(module, behavior_idx, init) }
    }

    fn send_message(
        &mut self,
        target: crate::vm::Value,
        behavior_id: u16,
        args: &[crate::vm::Value],
    ) {
        if let Some(target_id) = target.as_actor_id() {
            // SAFETY: as above. `send_message_by_id` is safe mid-behavior:
            // it pushes mail, bumps ORCA foreign counts, and enqueues the
            // target; the receive-wait wake is deferred while the shared
            // VM is executing (see `Runtime::pending_receive_wakes`).
            unsafe { (*self.runtime).send_message_by_id(target_id, behavior_id, args) }
        }
    }

    fn get_state_field(&self, field: &str) -> crate::vm::Value {
        unsafe {
            if let Some(actor) = (*self.runtime).actors.get(&self.actor_id) {
                return actor
                    .get_state_field(field)
                    .unwrap_or(crate::vm::Value::nil());
            }
        }
        crate::vm::Value::nil()
    }

    fn set_state_field(&mut self, field: &str, value: crate::vm::Value) {
        unsafe {
            if let Some(actor) = (*self.runtime).actors.get_mut(&self.actor_id) {
                // CRDT-backed fields mutate only through the `Crdt.*` effect
                // module; a raw `self.field = expr` assignment is ignored so it
                // cannot silently orphan `state_data` from the replicated entry.
                if actor
                    .state_models
                    .get(field)
                    .map(|m| m.is_crdt())
                    .unwrap_or(false)
                {
                    return;
                }
                actor.set_state_field(field, value);
            }
        }
    }

    fn emit_event(&mut self, event: &str, args: &[crate::vm::Value]) {
        unsafe {
            (*self.runtime).emit_event(self.actor_id, event, args);
        }
    }

    fn wait_signal(&mut self, name: &str) -> crate::vm::SignalWaitResult {
        unsafe {
            if let Some(actor) = (*self.runtime).actors.get(&self.actor_id) {
                if actor.received_signals.iter().any(|(n, _)| n == name) {
                    return crate::vm::SignalWaitResult::Ready(crate::vm::Value::unit());
                }
            }
            crate::vm::SignalWaitResult::NotReady
        }
    }

    fn suspend_for_signal(&mut self, _name: &str, _vm_state: Option<crate::vm::SuspendedVmState>) {
        // State capture is handled by run_bytecode_at_offset after run_from
        // returns, avoiding aliasing the Runtime through this raw-pointer
        // callback while the VM borrow is active.
    }

    fn perform_effect(
        &mut self,
        effect_name: &str,
        regs: &[crate::vm::Value],
    ) -> Option<crate::vm::Value> {
        unsafe {
            if effect_name != "Timer" {
                return None;
            }
            let actor = (*self.runtime).actors.get(&self.actor_id)?;
            if !actor.is_workflow {
                return Some(crate::vm::Value::unit());
            }
            let vm = (*self.runtime).vm.as_mut()?;
            let module_idx = vm.current_module_idx()?;
            let string_id = regs.get(0)?.as_string_id()?;
            let name = vm.constant_string(module_idx, string_id)?;
            let duration_ms = regs.get(1)?.as_int()? as u64;
            (*self.runtime).schedule_workflow_timer(self.actor_id, &name, duration_ms);
            Some(crate::vm::Value::unit())
        }
    }

    #[cfg_attr(not(feature = "ai-runtime"), allow(unused_variables))]
    fn perform_builtin_effect(
        &mut self,
        effect_name: &str,
        op_name: Option<&str>,
        constants: &[crate::bytecode::Constant],
        regs: &[crate::vm::Value],
    ) -> Option<crate::vm::Value> {
        unsafe {
            if effect_name == "Workflow" && op_name == Some("query") {
                let workflow_id = regs.get(0)?.as_actor_id()?;
                let string_id = regs.get(1)?.as_string_id()?;
                let query_name = match constants.get(string_id as usize) {
                    Some(crate::bytecode::Constant::String(s)) => s.clone(),
                    _ => return None,
                };
                return (*self.runtime).query_workflow(workflow_id, &query_name);
            }
            #[cfg(feature = "sqlite")]
            if effect_name == "DB" && op_name == Some("query") {
                let sql = match regs.first().and_then(|v| v.as_string_id()) {
                    Some(id) => match constants.get(id as usize) {
                        Some(crate::bytecode::Constant::String(s)) => s.clone(),
                        _ => return Some(crate::vm::Value::nil()),
                    },
                    None => return Some(crate::vm::Value::nil()),
                };
                let params: Vec<crate::vm::Value> = regs.iter().skip(1).copied().collect();
                return match (*self.runtime).persistence.query(&sql, &params) {
                    Ok(rows) => {
                        let json = serde_json::to_string(&rows).unwrap_or_default();
                        if let Some(vm) = &mut (*self.runtime).vm {
                            Some(vm.allocate_string(&json))
                        } else {
                            Some(crate::vm::Value::nil())
                        }
                    }
                    Err(_) => Some(crate::vm::Value::nil()),
                };
            }
            if effect_name == "Actor" {
                return (*self.runtime).perform_actor_builtin(
                    Some(self.actor_id),
                    op_name,
                    constants,
                    regs,
                );
            }

            if effect_name == "Crdt" {
                return (*self.runtime).perform_crdt_builtin(
                    Some(self.actor_id),
                    op_name,
                    constants,
                    regs,
                );
            }

            if effect_name == "Int" && op_name == Some("to_float") {
                let n = regs.first().and_then(|v| v.as_int()).unwrap_or(0);
                return Some(crate::vm::Value::float(n as f64));
            }
            if effect_name == "Float" && op_name == Some("to_int") {
                let x = regs.first().and_then(|v| v.as_float()).unwrap_or(0.0);
                return Some(crate::vm::Value::int(x as i64));
            }
            if effect_name == "Float" && op_name == Some("to_string") {
                let x = regs.first().and_then(|v| v.as_float()).unwrap_or(0.0);
                let s = format!("{}", x);
                if let Some(vm) = &mut (*self.runtime).vm {
                    return Some(vm.allocate_string(&s));
                }
                return Some(crate::vm::Value::nil());
            }
            if effect_name == "String" && op_name == Some("to_int") {
                let s = crate::vm::resolve_value_string(
                    constants,
                    *regs.first().unwrap_or(&crate::vm::Value::nil()),
                );
                let n: i64 = s.parse().unwrap_or(0);
                return Some(crate::vm::Value::int(n));
            }
            if effect_name == "String" && op_name == Some("to_float") {
                let s = crate::vm::resolve_value_string(
                    constants,
                    *regs.first().unwrap_or(&crate::vm::Value::nil()),
                );
                let f: f64 = s.parse().unwrap_or(0.0);
                return Some(crate::vm::Value::float(f));
            }
            if effect_name == "Timer" && op_name == Some("after") {
                let ms = regs.first().and_then(|v| v.as_int()).unwrap_or(0);
                if ms > 0 {
                    let callback_id = regs.get(1).and_then(|v| v.as_string_id());
                    let callback_name = callback_id.and_then(|id| {
                        constants.get(id as usize).and_then(|c| match c {
                            crate::bytecode::Constant::String(s) => Some(s.clone()),
                            _ => None,
                        })
                    });
                    if let Some(callback_name) = callback_name {
                        let behavior_id = (*self.runtime)
                            .behavior_id_for(self.actor_id, &callback_name)
                            .unwrap_or(0);
                        if behavior_id > 0 {
                            (*self.runtime).timer_wheel.send_after(
                                std::time::Duration::from_millis(ms as u64),
                                self.actor_id,
                                behavior_id,
                                vec![],
                            );
                        }
                    }
                }
                return Some(crate::vm::Value::unit());
            }
            if effect_name == "Int" && op_name == Some("to_string") {
                let n = regs.first().and_then(|v| v.as_int()).unwrap_or(0);
                let s = format!("{}", n);
                if let Some(vm) = &mut (*self.runtime).vm {
                    return Some(vm.allocate_string(&s));
                }
                return Some(crate::vm::Value::nil());
            }

            if effect_name == "String" && op_name == Some("length") {
                let s = crate::vm::resolve_value_string(
                    constants,
                    *regs.first().unwrap_or(&crate::vm::Value::nil()),
                );
                return Some(crate::vm::Value::int(s.len() as i64));
            }
            if effect_name == "String" && op_name == Some("charAt") {
                let s = crate::vm::resolve_value_string(
                    constants,
                    *regs.first().unwrap_or(&crate::vm::Value::nil()),
                );
                let idx = regs.get(1).and_then(|v| v.as_int()).unwrap_or(-1);
                if idx < 0 || idx as usize >= s.len() {
                    return Some(crate::vm::Value::int(-1));
                }
                return Some(crate::vm::Value::int(s.as_bytes()[idx as usize] as i64));
            }
            if effect_name == "Provider" && op_name == Some("ask") {
                // General runtime-registered provider dispatch (actor path).
                // Mirrors RuntimeVmCallbacks::perform_builtin_effect's Provider
                // branch. The "llm" provider reuses the agent-aware complete_llm.
                let provider = match regs.get(0).and_then(|v| v.as_string_id()) {
                    Some(id) => match constants.get(id as usize) {
                        Some(crate::bytecode::Constant::String(s)) => s.clone(),
                        _ => return None,
                    },
                    None => return None,
                };
                let prompt = match regs.get(1) {
                    Some(v) => {
                        if let Some(id) = v.as_string_id() {
                            constants
                                .get(id as usize)
                                .and_then(|c| match c {
                                    crate::bytecode::Constant::String(s) => Some(s.clone()),
                                    _ => None,
                                })
                                .unwrap_or_default()
                        } else {
                            v.to_string_repr()
                        }
                    }
                    None => return None,
                };
                if provider == "llm" {
                    #[cfg(feature = "ai-runtime")]
                    {
                        let content = self.complete_llm("", &prompt);
                        let rt = &mut *self.runtime;
                        return Some(match content {
                            Some(c) => match &mut rt.vm {
                                Some(vm) => vm.allocate_string(&c),
                                None => crate::vm::Value::nil(),
                            },
                            None => crate::vm::Value::nil(),
                        });
                    }
                    #[cfg(not(feature = "ai-runtime"))]
                    {
                        return Some(crate::vm::Value::nil());
                    }
                }
            }
            if effect_name == "Debug" && op_name == Some("inspect") {
                let target_id = regs.first().and_then(|v| v.as_int()).unwrap_or(0) as u64;
                let rt = &mut *self.runtime;
                let info = serde_json::json!({
                    "state": rt.actors.get(&target_id).map(|a| {
                        a.state_data.iter().map(|(k, v)| {
                            (k.clone(), crate::vm::resolve_value_string(constants, *v))
                        }).collect::<std::collections::HashMap<_, _>>()
                    }).unwrap_or_default(),
                    "mailbox_size": rt.actors.get(&target_id).map(|a| a.mailbox.len()).unwrap_or(0),
                    "behaviors": rt.actors.get(&target_id).map(|a| {
                        a.behavior_table.iter().map(|b| b.name.clone()).collect::<Vec<_>>()
                    }).unwrap_or_default(),
                    "supervisor": rt.supervisors.get(&target_id).map(|_s| target_id),
                });
                let json = serde_json::to_string(&info).unwrap_or_default();
                if let Some(vm) = &mut rt.vm {
                    return Some(vm.allocate_string(&json));
                }
                return Some(crate::vm::Value::nil());
            }
            if effect_name == "IO" {
                if let (Some("print") | Some("println"), Some(first)) = (op_name, regs.first()) {
                    let msg = crate::vm::resolve_value_string(constants, *first);
                    println!("{}", msg);
                    return Some(crate::vm::Value::unit());
                }
            }
            self.perform_effect(effect_name, regs)
        }
    }

    fn perform_builtin_effect_in_module(
        &mut self,
        effect_name: &str,
        op_name: Option<&str>,
        module: &crate::bytecode::CodeModule,
        regs: &[crate::vm::Value],
    ) -> Option<crate::vm::Value> {
        let qualified = match op_name {
            Some(op) => format!("{}.{}", effect_name, op),
            None => effect_name.to_string(),
        };
        unsafe {
            // Check test handlers before real dispatch.
            if let Some(result) = (*self.runtime).check_test_handler(&qualified, regs) {
                return Some(result);
            }
            if effect_name == "Otp" {
                return (*self.runtime).perform_otp_builtin(op_name, module, regs);
            }
            if effect_name == "Http" && op_name == Some("serve") {
                let port = regs.first().and_then(|v| v.as_int()).unwrap_or(0) as u16;
                let func_idx = match regs.get(1) {
                    Some(v) if v.is_closure() => {
                        let payload = v.as_raw() & crate::value_layout::PAYLOAD_MASK;
                        if payload & crate::vm::CLOSURE_ENV_FLAG != 0 {
                            return Some(crate::vm::Value::nil());
                        }
                        payload as usize
                    }
                    Some(v) => v.as_int().unwrap_or(0) as usize,
                    None => return Some(crate::vm::Value::nil()),
                };
                return match HttpServerState::bind(port, module.clone(), func_idx) {
                    Ok(server) => {
                        let actual_port = server.port;
                        (*self.runtime).http_server = Some(server);
                        Some(crate::vm::Value::int(actual_port as i64))
                    }
                    Err(_) => Some(crate::vm::Value::nil()),
                };
            }
            self.perform_builtin_effect(effect_name, op_name, &module.constants, regs)
        }
    }

    #[cfg(feature = "ai-runtime")]
    fn complete_llm(&mut self, model: &str, prompt: &str) -> Option<String> {
        unsafe {
            let rt = &mut *self.runtime;
            if rt
                .actors
                .get(&self.actor_id)
                .map(|a| a.is_agent)
                .unwrap_or(false)
            {
                return rt.complete_agent_llm(self.actor_id, prompt);
            }
            let request = rt.build_actor_llm_request(self.actor_id, model, prompt)?;
            let module = rt.actors.get(&self.actor_id)?.bytecode_module.clone()?;
            rt.complete_llm_with_tools(request, Vec::new(), &module)
                .ok()?
                .content
        }
    }

    #[cfg(feature = "ai-runtime")]
    fn llm_ask(&mut self, model: &str, prompt: &str) -> crate::vm::PerformAsyncResult {
        use crate::vm::PerformAsyncResult;
        unsafe {
            let rt = &mut *self.runtime;
            let actor_id = self.actor_id;

            // Nested synchronous paths (pipelines, ask_actor_sync) keep the
            // blocking behavior.
            if !rt.suspend_enabled {
                return PerformAsyncResult::Ready(self.complete_llm(model, prompt));
            }

            // Re-executed after a resume: a completed response is waiting.
            let completed = rt
                .actors
                .get_mut(&actor_id)
                .and_then(|actor| actor.llm_completed.take());
            if let Some(result) = completed {
                return match result {
                    Ok(response) => {
                        // Finish on the scheduler thread: tool invocation and
                        // durable-state write-back must not run on the worker.
                        let prev_current_actor = rt.current_actor;
                        rt.current_actor = Some(actor_id);
                        let is_agent = rt
                            .actors
                            .get(&actor_id)
                            .map(|a| a.is_agent)
                            .unwrap_or(false);
                        let content = if is_agent {
                            let module = rt
                                .actors
                                .get(&actor_id)
                                .and_then(|a| a.bytecode_module.clone());
                            let processed = match module {
                                Some(m) => rt.finish_tool_calls(&m, response),
                                None => Ok(response),
                            };
                            match processed {
                                Ok(resp) => agent::finish_agent_llm(rt, actor_id, prompt, &resp),
                                Err(_) => None,
                            }
                        } else {
                            let module = rt
                                .actors
                                .get(&actor_id)
                                .and_then(|a| a.bytecode_module.clone());
                            match module {
                                Some(m) => rt
                                    .finish_tool_calls(&m, response)
                                    .ok()
                                    .and_then(|r| r.content),
                                None => response.content,
                            }
                        };
                        rt.current_actor = prev_current_actor;
                        PerformAsyncResult::Ready(content)
                    }
                    Err(_) => PerformAsyncResult::Ready(None),
                };
            }

            // A call is already in flight (defensive; should not happen).
            if rt
                .actors
                .get(&actor_id)
                .map(|a| a.llm_inflight)
                .unwrap_or(false)
            {
                return PerformAsyncResult::Pending;
            }

            // Build the request on the scheduler thread, then hand it to a
            // background worker for the HTTP call.
            let is_agent = rt
                .actors
                .get(&actor_id)
                .map(|a| a.is_agent)
                .unwrap_or(false);
            let request = if is_agent {
                agent::build_agent_llm_request(rt, actor_id, prompt)
            } else {
                rt.build_actor_llm_request(actor_id, model, prompt)
            };
            // Build failure (e.g. missing agent state fields): nil response.
            let Some(request) = request else {
                return PerformAsyncResult::Ready(None);
            };
            if !(*rt).dispatch_llm_request(actor_id, request, prompt) {
                // Dispatch failed: fall back to a nil response.
                rt.llm.inflight_count = rt.llm.inflight_count.saturating_sub(1);
                if let Some(actor) = rt.actors.get_mut(&actor_id) {
                    actor.llm_inflight = false;
                    actor.llm_pending_prompt = None;
                }
                return PerformAsyncResult::Ready(None);
            }
            PerformAsyncResult::Pending
        }
    }

    #[cfg_attr(not(feature = "ai-runtime"), allow(unused_variables))]
    fn perform_async(
        &mut self,
        effect_op: &str,
        constants: &[crate::bytecode::Constant],
        args: &[crate::vm::Value],
    ) -> crate::vm::PerformAsyncResult {
        use crate::vm::PerformAsyncResult;
        match effect_op {
            #[cfg(feature = "ai-runtime")]
            "Inference.ask" | "LLM.ask" => {
                let prompt = resolve_first_string(constants, args);
                self.llm_ask("", &prompt)
            }
            "Timer.sleep" => {
                let ms = args.first().and_then(|v| v.as_int()).unwrap_or(0) as u64;
                unsafe {
                    let rt = &mut *self.runtime;
                    if let Some(actor) = rt.actors.get_mut(&self.actor_id) {
                        if actor.timer_sleep_fired {
                            actor.timer_sleep_fired = false;
                            return PerformAsyncResult::Ready(None);
                        }
                    }
                    if ms == 0 {
                        return PerformAsyncResult::Ready(None);
                    }
                    if ms > 0 {
                        rt.timer_wheel
                            .timer_sleep_wake(std::time::Duration::from_millis(ms), self.actor_id);
                    }
                }
                PerformAsyncResult::Pending
            }
            #[cfg(feature = "ai-runtime")]
            "Pipeline.new" => {
                let id = unsafe { (*self.runtime).pipeline_new() };
                PerformAsyncResult::Ready(Some(id.to_string()))
            }
            #[cfg(feature = "ai-runtime")]
            "Pipeline.stage" => {
                let id = id_arg(constants, args, 0);
                let name = string_arg(constants, args, 1);
                let actor = actor_arg(args, 2);
                let template = string_arg(constants, args, 3);
                let result = unsafe { (*self.runtime).pipeline_stage(id, &name, actor, &template) };
                let r = result.map(|id| id as i64).unwrap_or(-1);
                PerformAsyncResult::Ready(Some(r.to_string()))
            }
            #[cfg(feature = "ai-runtime")]
            "Pipeline.run" => {
                let id = id_arg(constants, args, 0);
                let input = string_arg(constants, args, 1);
                let result = unsafe { (*self.runtime).pipeline_run(id, &input).ok() };
                PerformAsyncResult::Ready(result)
            }
            #[cfg(feature = "ai-runtime")]
            "Supervisor.new" => {
                let id = unsafe { (*self.runtime).supervisor_new() };
                PerformAsyncResult::Ready(Some(id.to_string()))
            }
            #[cfg(feature = "ai-runtime")]
            "Supervisor.worker" => {
                let id = id_arg(constants, args, 0);
                let name = string_arg(constants, args, 1);
                let actor = actor_arg(args, 2);
                let description = string_arg(constants, args, 3);
                let result =
                    unsafe { (*self.runtime).supervisor_worker(id, &name, actor, &description) };
                let r = result.map(|id| id as i64).unwrap_or(-1);
                PerformAsyncResult::Ready(Some(r.to_string()))
            }
            #[cfg(feature = "ai-runtime")]
            "Supervisor.run" => {
                let id = id_arg(constants, args, 0);
                let task = string_arg(constants, args, 1);
                let result = unsafe { (*self.runtime).supervisor_run(id, &task).ok() };
                PerformAsyncResult::Ready(result)
            }
            #[cfg(feature = "ai-runtime")]
            "Debate.new" => {
                let topic = string_arg(constants, args, 0);
                let rounds = int_arg(args, 1);
                let threshold = float_arg(args, 2);
                let id = unsafe { (*self.runtime).debate_new(&topic, rounds, threshold) };
                PerformAsyncResult::Ready(Some(id.to_string()))
            }
            #[cfg(feature = "ai-runtime")]
            "Debate.participant" => {
                let id = id_arg(constants, args, 0);
                let name = string_arg(constants, args, 1);
                let stance = string_arg(constants, args, 2);
                let actor = actor_arg(args, 3);
                let result =
                    unsafe { (*self.runtime).debate_participant(id, &name, &stance, actor) };
                let r = result.map(|id| id as i64).unwrap_or(-1);
                PerformAsyncResult::Ready(Some(r.to_string()))
            }
            #[cfg(feature = "ai-runtime")]
            "Debate.run" => {
                let id = id_arg(constants, args, 0);
                let result = unsafe { (*self.runtime).debate_run(id).ok() };
                PerformAsyncResult::Ready(result)
            }
            _ => PerformAsyncResult::Ready(None),
        }
    }

    fn try_receive(&mut self) -> Option<(u16, crate::vm::Value)> {
        unsafe {
            let msg = {
                let actor = (*self.runtime).actors.get_mut(&self.actor_id)?;
                actor.mailbox.pop()?
            };
            // ORCA receiver protocol: hold heap pointers carried by the message.
            (*self.runtime).hold_payload_refs(self.actor_id, &*msg.payload);
            let val = msg
                .payload
                .first()
                .cloned()
                .unwrap_or(crate::vm::Value::unit());
            Some((msg.behavior_id, val))
        }
    }

    fn try_receive_match(
        &mut self,
        behavior_ids: &[u16],
    ) -> Option<(usize, Vec<crate::vm::Value>)> {
        unsafe {
            let (pos, payload) = {
                let actor = (*self.runtime).actors.get_mut(&self.actor_id)?;
                actor.mailbox.receive_match(behavior_ids)?
            };
            // ORCA receiver protocol: hold heap pointers carried by the message.
            (*self.runtime).hold_payload_refs(self.actor_id, &*payload);
            Some((
                pos,
                Arc::try_unwrap(payload).unwrap_or_else(|arc| (*arc).clone()),
            ))
        }
    }

    fn receive_wait_suspend(&mut self, timeout_ms: i64) -> bool {
        unsafe {
            let rt = &mut *self.runtime;
            let Some(actor) = rt.actors.get_mut(&self.actor_id) else {
                return false;
            };
            // A fired timeout resolves the wait exactly once: consume the
            // marker so the re-executed ReceiveWait writes the no-match
            // sentinel and a later wait starts clean.
            if actor.receive_wait.map(|w| w.timed_out).unwrap_or(false) {
                actor.receive_wait = None;
                return false;
            }
            // Non-positive timeouts poll once (Erlang-style non-blocking
            // receive). Synchronous entry points (ask_actor_sync: pipelines,
            // supervisors, debates, `Ask`) never suspend — same gating as
            // the non-blocking LLM path.
            if timeout_ms <= 0 || !rt.suspend_enabled {
                return false;
            }
            true
        }
    }

    fn receive_wait_matched(&mut self) {
        unsafe {
            let rt = &mut *self.runtime;
            let wait = rt
                .actors
                .get_mut(&self.actor_id)
                .and_then(|a| a.receive_wait.take());
            // A match resolves the wait: cancel the pending timeout so it
            // cannot fire into a later wait on this actor.
            if let Some(wait) = wait {
                rt.timer_wheel.cancel(wait.timer_id);
            }
        }
    }

    fn commit_receive_match(&mut self) {
        unsafe {
            if let Some(actor) = (*self.runtime).actors.get_mut(&self.actor_id) {
                actor.mailbox.commit_receive_match();
            }
        }
    }

    fn reset_receive_match(&mut self) {
        unsafe {
            if let Some(actor) = (*self.runtime).actors.get_mut(&self.actor_id) {
                actor.mailbox.reset_receive_match();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Distributed callbacks for the bytecode VM — bridges RSend/RAsk/RSpawn
// opcodes to the runtime's send_distributed infrastructure.
// ---------------------------------------------------------------------------

/// Raw-pointer callbacks for distributed VM opcodes (`RSend`, `RAsk`,
/// `Migrate`, `RSpawn`, `Gossip`).  Mirrors [`BytecodeRuntimeCallbacks`]
/// in using a transient `*mut Runtime` borrow — the VM calls these only
/// while the runtime holds `&mut self`, so the pointer is valid and unique.
#[derive(Debug)]
pub(crate) struct BytecodeDistributedCallbacks {
    pub(crate) runtime: *mut Runtime,
}

// SAFETY: the VM only invokes these callbacks while the calling
// `Runtime` method holds `&mut self`.  The raw pointer is therefore the
// sole active borrow of the runtime.
unsafe impl Send for BytecodeDistributedCallbacks {}
unsafe impl Sync for BytecodeDistributedCallbacks {}

impl crate::vm::DistributedVmCallbacks for BytecodeDistributedCallbacks {
    fn node_id(&self) -> u64 {
        unsafe {
            (*self.runtime)
                .distributed
                .node_id
                .map(|n| n.0)
                .unwrap_or(0)
        }
    }

    fn remote_send(
        &mut self,
        target_actor: u64,
        target_node: u64,
        behavior: &str,
        args: &[crate::vm::Value],
    ) {
        unsafe {
            let rt = &mut *self.runtime;
            let node_id = rt.distributed.node_id.map(|n| n.0).unwrap_or(0);
            // If the target is the local node, or distributed transport is
            // not available, fall back to local delivery instead of silently
            // dropping the message.
            if target_node == node_id
                || rt.distributed.transport.is_none()
                || rt.distributed.cluster.is_none()
                || rt.distributed.resolver.is_none()
            {
                rt.send_message(target_actor, behavior, args);
                return;
            }
            // Take distributed fields out so send_distributed can borrow
            // them independently of rt itself.
            let mut transport = rt.distributed.transport.take();
            let mut resolver = rt.distributed.resolver.take();
            let cluster = rt.distributed.cluster.take();
            if let (Some(ref mut t), Some(ref c), Some(ref mut r)) =
                (&mut transport, &cluster, &mut resolver)
            {
                let target = ActorAddress::remote(NodeId(target_node), target_actor);
                send_distributed(rt, t, c, r, target, behavior, args);
            }
            rt.distributed.transport = transport;
            rt.distributed.resolver = resolver;
            rt.distributed.cluster = cluster;
        }
    }

    fn migrate(&mut self, actor_id: u64, target_node_id: u64) {
        unsafe {
            let rt = &mut *self.runtime;
            let target = NodeId(target_node_id);

            // Extract all needed data from the actor in a tight scope so the
            // immutable borrow on rt.actors is released before reap_living_actor
            // takes a mutable borrow on rt.
            let (snapshot_json, nbc_bytes) = {
                let actor = match rt.actors.get(&actor_id) {
                    Some(a) => a,
                    None => {
                        tracing::warn!(
                            "nulang-migrate: actor {} not found for migration to {:?}",
                            actor_id,
                            target
                        );
                        return;
                    }
                };

                // Build the durable-state snapshot.
                let mut state = std::collections::HashMap::new();
                for (name, value) in &actor.state_data {
                    let model = actor
                        .state_models
                        .get(name)
                        .copied()
                        .unwrap_or(crate::runtime::persistence::StateModel::Local);
                    if model == crate::runtime::persistence::StateModel::Durable || model.is_crdt()
                    {
                        let persisted = if name == "semantic_memory" || name == "procedural_memory"
                        {
                            crate::runtime::workflow::vm_value_to_string_in_actor(
                                    value, actor,
                                )
                                .map(crate::runtime::persistence::PersistedValue::String)
                                .unwrap_or_else(|| {
                                    crate::runtime::persistence::PersistedValue::from_value_resolved(
                                        value,
                                        actor.bytecode_module.as_ref(),
                                    )
                                })
                        } else {
                            crate::runtime::persistence::PersistedValue::from_value_resolved(
                                value,
                                actor.bytecode_module.as_ref(),
                            )
                        };
                        state.insert(name.clone(), persisted);
                    }
                }

                // Snapshot global CRDT state.
                let crdt_snapshot = rt.crdt_manager.as_ref().map(|m| {
                    m.snapshot()
                        .into_iter()
                        .map(|(id, (ty, bytes))| (id.0, ty.to_u8(), bytes))
                        .collect()
                });

                let snapshot = crate::runtime::persistence::ActorSnapshot {
                    actor_id,
                    sequence: actor.sequence,
                    state,
                    waiting_signal: actor.waiting_signal.clone(),
                    crdt_snapshot,
                };

                let snapshot_json = match serde_json::to_vec(&snapshot) {
                    Ok(json) => json,
                    Err(e) => {
                        tracing::warn!(
                            "nulang-migrate: failed to serialize snapshot for actor {}: {}",
                            actor_id,
                            e
                        );
                        return;
                    }
                };

                // Get NBC-encoded bytecode module.
                let module = match actor.bytecode_module.as_ref() {
                    Some(m) => m.clone(),
                    None => match rt.recovery_modules.get(&actor_id) {
                        Some((m, _, _)) => m.clone(),
                        None => {
                            tracing::warn!(
                                "nulang-migrate: no bytecode module for actor {}",
                                actor_id
                            );
                            return;
                        }
                    },
                };
                let nbc = match module.to_nbc(None) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        tracing::warn!(
                            "nulang-migrate: failed to encode NBC for actor {}: {}",
                            actor_id,
                            e
                        );
                        return;
                    }
                };

                (snapshot_json, nbc)
            }; // <- actor borrow released here

            // Send the migration packet.
            let target_addr = rt
                .distributed
                .cluster
                .as_ref()
                .and_then(|c| c.get_node(target))
                .map(|info| info.address);

            let packet = super::network::Packet::MigrateActor {
                actor_id,
                nbc_bytes,
                snapshot_json,
            };

            if let (Some(transport), Some(addr)) = (&mut rt.distributed.transport, target_addr) {
                transport.send(target, addr, packet);
            } else {
                tracing::warn!(
                    "nulang-migrate: cannot reach target node {:?} for actor {}",
                    target,
                    actor_id
                );
                return;
            }

            // Register forwarding entry BEFORE reaping.
            rt.migrated_actors
                .insert(actor_id, (target, std::time::Instant::now()));

            // Reap the actor cleanly (no supervisor restart — migration is
            // intentional relocation, not a crash).
            crate::runtime::exit::reap_living_actor(rt, actor_id, crate::types::ExitReason::Normal);

            tracing::info!(
                "nulang-migrate: actor {} migrated to node {:?}",
                actor_id,
                target
            );
        }
    }
    fn remote_ask(
        &mut self,
        target_actor: u64,
        behavior: &str,
        args: &[crate::vm::Value],
        _timeout_ms: u64,
    ) -> crate::vm::Value {
        // Send the ask request over the network. The reply is expected to
        // arrive via the normal message path (the target actor sends back
        // a response message). The caller should use `receive` to collect
        // the reply. Full suspend/resume support (RFC 0007) would block
        // the actor until the reply or timeout.
        unsafe {
            let rt = &mut *self.runtime;
            // Cross-node routing by bare actor-ref value: if the target id
            // is a known remote ref (spawn@node placeholder or inbound
            // sender), route to ITS node; otherwise fall back to the
            // local-node path (single-node `ask remote` local delivery).
            match rt.remote_refs.get(&target_actor).copied() {
                Some(node) => {
                    rt.route_ref_send(target_actor, node, behavior, args);
                }
                None => {
                    let node_id = rt.distributed.node_id.map(|n| n.0).unwrap_or(0);
                    let target =
                        ActorAddress::remote(crate::runtime::NodeId(node_id), target_actor);
                    rt.send_distributed(target, behavior, args);
                }
            }
        }
        crate::vm::Value::nil()
    }
    fn remote_spawn(
        &mut self,
        target_node: u64,
        behavior: &str,
        init: &[(String, crate::vm::Value)],
    ) -> crate::vm::Value {
        unsafe {
            let rt = &mut *self.runtime;
            let node = NodeId(target_node);
            let mut transport = rt.distributed.transport.take();
            let mut resolver = rt.distributed.resolver.take();
            let cluster = rt.distributed.cluster.take();
            let result = if let (Some(ref mut t), Some(ref c), Some(ref mut r)) =
                (&mut transport, &cluster, &mut resolver)
            {
                let addr = spawn_on_node(rt, t, c, r, node, behavior, init.to_vec());
                crate::vm::Value::actor_ref(addr.actor_id())
            } else {
                crate::vm::Value::actor_ref(0)
            };
            rt.distributed.transport = transport;
            rt.distributed.resolver = resolver;
            rt.distributed.cluster = cluster;
            result
        }
    }
    fn gossip(&mut self, _message: &str) -> crate::vm::Value {
        crate::vm::Value::unit()
    }
}

// ---------------------------------------------------------------------------
// Argument extraction helpers (shared by async-effect dispatch)
// ---------------------------------------------------------------------------

/// Resolve the first argument as a string (via constant-pool lookup or the
/// value's own string representation).
#[cfg(feature = "ai-runtime")]
fn resolve_first_string(constants: &[crate::bytecode::Constant], args: &[crate::vm::Value]) -> String {
    args.first()
        .map(|v| crate::vm::resolve_value_string(constants, *v))
        .unwrap_or_default()
}

/// Extract a string argument at `idx`, resolving string-constant IDs
/// through the module constant pool.
#[cfg(feature = "ai-runtime")]
fn string_arg(
    constants: &[crate::bytecode::Constant],
    args: &[crate::vm::Value],
    idx: usize,
) -> String {
    args.get(idx)
        .map(|v| crate::vm::resolve_value_string(constants, *v))
        .unwrap_or_default()
}

/// Extract an actor-id argument at `idx`.
#[cfg(feature = "ai-runtime")]
fn actor_arg(args: &[crate::vm::Value], idx: usize) -> u64 {
    args.get(idx)
        .and_then(|v| v.as_actor_id())
        .unwrap_or(0)
}

/// Extract an integer argument at `idx` (accepts tagged Int or a string that
/// parses as an integer).
#[cfg(feature = "ai-runtime")]
fn int_arg(args: &[crate::vm::Value], idx: usize) -> i64 {
    match args.get(idx) {
        Some(v) => v.as_int().unwrap_or_else(|| {
            v.to_string_repr().parse().unwrap_or(0)
        }),
        None => 0,
    }
}

/// Extract a float argument at `idx`.
#[cfg(feature = "ai-runtime")]
fn float_arg(args: &[crate::vm::Value], idx: usize) -> f64 {
    match args.get(idx) {
        Some(v) => v.as_float().unwrap_or_else(|| {
            v.to_string_repr().parse().unwrap_or(0.0)
        }),
        None => 0.0,
    }
}

/// Extract an id argument from a string or int.
#[cfg(feature = "ai-runtime")]
fn id_arg(constants: &[crate::bytecode::Constant], args: &[crate::vm::Value], idx: usize) -> String {
    string_arg(constants, args, idx)
}
