//! Standard-library inventory.
//!
//! This module is an inventory/documentation layer only: it does not
//! implement any behavior. It records every built-in effect operation and
//! function that is currently wired into the VM and runtime, so tools
//! (REPL, LSP, docs generators) have a single source of truth for what a
//! `perform Effect.op(...)` call resolves to when no user handler is
//! installed.
//!
//! The wiring itself lives elsewhere:
//! - `IO.print` / `IO.println` / `IO.read`: `VM::perform_builtin_effect`
//!   in `vm.rs` (standalone, actor-free scripts).
//! - `Timer.sleep`: the runtime host's `perform_effect` callback in
//!   `runtime/mod.rs` (workflow actors only).
//! - `Signal.wait`: lowered to the `SignalWait` opcode in `mir_lower.rs`,
//!   served by the host `wait_signal` callback.
//! - `LLM.ask`: lowered to the `LlmAsk` opcode in `mir_lower.rs`, served
//!   by the host `llm_ask` / `complete_llm` callbacks.
//! - `Actor.*` (link/unlink/monitor/demonitor/trap_exit/exit/register/
//!   unregister/whereis/set_priority): `Runtime::perform_actor_builtin` in
//!   `runtime/mod.rs`, reached through both runtime host callback impls;
//!   the standalone VM answers them with a nil no-op.
//! - `Otp.*` (create_supervisor/supervise_child/set_template/start_child/
//!   terminate_child/child_count): `Runtime::perform_otp_builtin` in
//!   `runtime/mod.rs`, reached through both runtime host callback impls;
//!   the standalone VM answers them with a nil no-op.

use crate::types::Span;
use crate::types::{NuError, NuResult};

// ---------------------------------------------------------------------------
// BuiltinOp: one built-in effect operation
// ---------------------------------------------------------------------------

/// Where a built-in operation is implemented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImplSite {
    /// Handled by `VM::perform_builtin_effect` in the standalone VM
    /// (actor-free scripts); no runtime required.
    StandaloneVm,
    /// Handled by a runtime host callback (`ActorVmCallbacks`); requires
    /// the actor runtime and, for `Timer.sleep`, a workflow actor.
    RuntimeHost,
}

/// A single built-in effect operation wired into the VM/runtime.
#[derive(Debug, Clone, Copy)]
pub struct BuiltinOp {
    /// Fully-qualified name as dispatched by the VM, e.g. `"IO.print"`.
    pub name: &'static str,
    /// Effect the operation belongs to, e.g. `"IO"`.
    pub effect: &'static str,
    /// Operation name within the effect, e.g. `"print"`.
    pub op: &'static str,
    /// Human-readable signature, e.g. `"print(msg: String) -> Unit"`.
    pub signature: &'static str,
    /// Where the operation is implemented.
    pub implemented_in: ImplSite,
    /// One-line description of the behavior.
    pub description: &'static str,
}

// ---------------------------------------------------------------------------
// StdLib: registry of built-in operations
// ---------------------------------------------------------------------------

/// Registry of every built-in effect operation currently wired into the
/// VM and runtime.
///
/// The registry is static: it mirrors the dispatch sites in `vm.rs` and
/// `runtime/mod.rs` and is updated by hand when a new built-in is wired.
pub struct StdLib {
    ops: Vec<BuiltinOp>,
}

impl StdLib {
    /// Build the registry with all currently wired built-ins.
    pub fn new() -> Self {
        StdLib {
            ops: vec![
                BuiltinOp {
                    name: "IO.print",
                    effect: "IO",
                    op: "print",
                    signature: "print(msg: String) -> Unit",
                    implemented_in: ImplSite::StandaloneVm,
                    description: "Write the argument to stdout, followed by a newline.",
                },
                BuiltinOp {
                    name: "IO.println",
                    effect: "IO",
                    op: "println",
                    signature: "println(msg: String) -> Unit",
                    implemented_in: ImplSite::StandaloneVm,
                    description: "Alias of `IO.print`; writes the argument to stdout with a newline.",
                },
                BuiltinOp {
                    name: "IO.read",
                    effect: "IO",
                    op: "read",
                    signature: "read() -> String",
                    implemented_in: ImplSite::StandaloneVm,
                    description: "Read one line from stdin; returns the line without the trailing newline.",
                },
                BuiltinOp {
                    name: "Int.to_string",
                    effect: "Int",
                    op: "to_string",
                    signature: "to_string(value: Int) -> String",
                    implemented_in: ImplSite::RuntimeHost,
                    description: "Convert an integer to its string representation.",
                },

                BuiltinOp {
                    name: "String.length",
                    effect: "String",
                    op: "length",
                    signature: "length(s: String) -> Int",
                    implemented_in: ImplSite::StandaloneVm,
                    description: "Return the length of the string in bytes.",
                },
                BuiltinOp {
                    name: "String.charAt",
                    effect: "String",
                    op: "charAt",
                    signature: "charAt(s: String, index: Int) -> Int",
                    implemented_in: ImplSite::StandaloneVm,
                    description: "Return the byte at the given index in the string, or -1 if out of bounds.",
                },
                BuiltinOp {
                    name: "Timer.sleep",
                    effect: "Timer",
                    op: "sleep",
                    signature: "sleep(name: String, duration_ms: Int) -> Unit",
                    implemented_in: ImplSite::RuntimeHost,
                    description: "Schedule a durable workflow timer; only available inside workflow actors.",
                },
                BuiltinOp {
                    name: "Signal.wait",
                    effect: "Signal",
                    op: "wait",
                    signature: "wait(name: String) -> Unit",
                    implemented_in: ImplSite::RuntimeHost,
                    description: "Suspend the workflow until the named signal arrives, then resume with unit.",
                },
                BuiltinOp {
                    name: "LLM.ask",
                    effect: "LLM",
                    op: "ask",
                    signature: "ask(prompt: String) -> String",
                    implemented_in: ImplSite::RuntimeHost,
                    description: "Send the prompt to the configured LLM client and return the reply; suspends non-blockingly when the runtime supports it.",
                },
                BuiltinOp {
                    name: "Actor.link",
                    effect: "Actor",
                    op: "link",
                    signature: "link(target: Actor) -> Nil",
                    implemented_in: ImplSite::RuntimeHost,
                    description: "Link the current actor to `target`; abnormal exits propagate to linked peers. Nil no-op outside an actor.",
                },
                BuiltinOp {
                    name: "Actor.unlink",
                    effect: "Actor",
                    op: "unlink",
                    signature: "unlink(target: Actor) -> Nil",
                    implemented_in: ImplSite::RuntimeHost,
                    description: "Remove the link between the current actor and `target`. Nil no-op outside an actor.",
                },
                BuiltinOp {
                    name: "Actor.monitor",
                    effect: "Actor",
                    op: "monitor",
                    signature: "monitor(target: Actor) -> Nil",
                    implemented_in: ImplSite::RuntimeHost,
                    description: "Monitor `target` from the current actor; a DOWN system message is delivered when it exits. Nil no-op outside an actor.",
                },
                BuiltinOp {
                    name: "Actor.demonitor",
                    effect: "Actor",
                    op: "demonitor",
                    signature: "demonitor(target: Actor) -> Nil",
                    implemented_in: ImplSite::RuntimeHost,
                    description: "Stop the current actor's monitor on `target`, so no DOWN message is delivered. Nil no-op outside an actor.",
                },
                BuiltinOp {
                    name: "Actor.trap_exit",
                    effect: "Actor",
                    op: "trap_exit",
                    signature: "trap_exit(flag: Bool) -> Nil",
                    implemented_in: ImplSite::RuntimeHost,
                    description: "Set the current actor's trap_exits flag; when true, linked-peer exit signals arrive as system messages instead of killing it. Nil no-op outside an actor.",
                },
                BuiltinOp {
                    name: "Actor.exit",
                    effect: "Actor",
                    op: "exit",
                    signature: "exit(reason: Int | String) -> Nil",
                    implemented_in: ImplSite::RuntimeHost,
                    description: "Self-exit the current actor; 0/\"normal\", 1/\"error\", 2/\"kill\" select the reason, anything else is custom. Nil no-op outside an actor.",
                },
                BuiltinOp {
                    name: "Actor.register",
                    effect: "Actor",
                    op: "register",
                    signature: "register(name: String) -> Nil",
                    implemented_in: ImplSite::RuntimeHost,
                    description: "Register the current actor under `name` in the local actor registry. Nil no-op outside an actor.",
                },
                BuiltinOp {
                    name: "Actor.unregister",
                    effect: "Actor",
                    op: "unregister",
                    signature: "unregister(name: String) -> Nil",
                    implemented_in: ImplSite::RuntimeHost,
                    description: "Remove `name` from the local actor registry.",
                },
                BuiltinOp {
                    name: "Actor.whereis",
                    effect: "Actor",
                    op: "whereis",
                    signature: "whereis(name: String) -> Actor | Nil",
                    implemented_in: ImplSite::RuntimeHost,
                    description: "Look up `name` in the local actor registry; returns the actor ref, or nil when the name is not registered.",
                },
                BuiltinOp {
                    name: "Actor.set_priority",
                    effect: "Actor",
                    op: "set_priority",
                    signature: "set_priority(level: Int) -> Nil",
                    implemented_in: ImplSite::RuntimeHost,
                    description: "Set the current actor's scheduling priority: 0=High, 1=Normal, 2=Low (any other value selects Normal). Ready High-priority actors are scheduled before Normal, Normal before Low; affects scheduling only, not message order. Nil no-op outside an actor.",
                },
                BuiltinOp {
                    name: "Otp.create_supervisor",
                    effect: "Otp",
                    op: "create_supervisor",
                    signature: "create_supervisor(name: String, strategy: Int) -> Int | Nil",
                    implemented_in: ImplSite::RuntimeHost,
                    description: "Create an OTP supervisor actor and return its id; strategy is 0=one_for_one, 1=one_for_all, 2=rest_for_one, 3=simple_one_for_one (any other value yields nil). Nil no-op outside a runtime.",
                },
                BuiltinOp {
                    name: "Otp.supervise_child",
                    effect: "Otp",
                    op: "supervise_child",
                    signature: "supervise_child(sup: Int, child: Actor, policy: Int) -> Nil",
                    implemented_in: ImplSite::RuntimeHost,
                    description: "Place an existing actor under a supervisor; policy is 0=permanent, 1=temporary, 2=transient (any other value is a no-op). Unknown supervisor ids are nil no-ops.",
                },
                BuiltinOp {
                    name: "Otp.set_template",
                    effect: "Otp",
                    op: "set_template",
                    signature: "set_template(sup: Int, type_name: String) -> Nil",
                    implemented_in: ImplSite::RuntimeHost,
                    description: "Set the child template of a simple_one_for_one supervisor to the named actor type, resolved against the performing module's actor metadata. Unknown types or supervisor ids are nil no-ops.",
                },
                BuiltinOp {
                    name: "Otp.start_child",
                    effect: "Otp",
                    op: "start_child",
                    signature: "start_child(sup: Int) -> Actor | Nil",
                    implemented_in: ImplSite::RuntimeHost,
                    description: "Spawn a fresh child from a simple_one_for_one supervisor's template and supervise it; returns the child actor ref, or nil when the supervisor is unknown, has no template, or is not simple_one_for_one.",
                },
                BuiltinOp {
                    name: "Otp.terminate_child",
                    effect: "Otp",
                    op: "terminate_child",
                    signature: "terminate_child(sup: Int, child: Actor) -> Nil",
                    implemented_in: ImplSite::RuntimeHost,
                    description: "Remove a child from supervision WITHOUT restarting it and exit it cleanly (Normal). Unknown supervisors or children are nil no-ops.",
                },
                BuiltinOp {
                    name: "Otp.child_count",
                    effect: "Otp",
                    op: "child_count",
                    signature: "child_count(sup: Int) -> Int | Nil",
                    implemented_in: ImplSite::RuntimeHost,
                    description: "Return the number of currently supervised children, or nil for an unknown supervisor id.",
                },
            ],
        }
    }

    /// All registered built-in operations, in registration order.
    pub fn ops(&self) -> &[BuiltinOp] {
        &self.ops
    }

    /// Look up a built-in by its fully-qualified name (e.g. `"IO.print"`).
    pub fn lookup(&self, name: &str) -> Option<&BuiltinOp> {
        self.ops.iter().find(|op| op.name == name)
    }

    /// Look up a built-in by fully-qualified name, or fail with a
    /// descriptive error naming the unknown operation.
    pub fn require(&self, name: &str) -> NuResult<&BuiltinOp> {
        self.lookup(name)
            .ok_or_else(|| NuError::RuntimeError { msg: format!("unknown built-in operation '{}'", name), span: Span::default() })
    }

    /// Distinct effect names covered by the registry, in first-seen order.
    pub fn effects(&self) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::new();
        for op in &self.ops {
            if !out.contains(&op.effect) {
                out.push(op.effect);
            }
        }
        out
    }
}

impl Default for StdLib {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// stdlib_docs: human-readable reference
// ---------------------------------------------------------------------------

/// Print a human-readable reference of every built-in effect operation
/// currently wired into the VM and runtime.
pub fn stdlib_docs() -> String {
    let lib = StdLib::new();
    let mut out = String::new();
    out.push_str("Nulang standard library — built-in effect operations\n");
    out.push_str("======================================================\n\n");
    for effect in lib.effects() {
        out.push_str(&format!("effect {}\n", effect));
        for op in lib.ops().iter().filter(|op| op.effect == effect) {
            let site = match op.implemented_in {
                ImplSite::StandaloneVm => "standalone VM",
                ImplSite::RuntimeHost => "runtime host",
            };
            out.push_str(&format!(
                "  {}  [{}]\n      {}\n",
                op.signature, site, op.description
            ));
        }
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_contains_expected_builtins() {
        let lib = StdLib::new();
        for name in [
            "IO.print",
            "IO.println",
            "IO.read",
            "Timer.sleep",
            "Signal.wait",
            "LLM.ask",
            "Actor.link",
            "Actor.unlink",
            "Actor.monitor",
            "Actor.demonitor",
            "Actor.trap_exit",
            "Actor.exit",
            "Actor.register",
            "Actor.unregister",
            "Actor.whereis",
            "Actor.set_priority",
            "Otp.create_supervisor",
            "Otp.supervise_child",
            "Otp.set_template",
            "Otp.start_child",
            "Otp.terminate_child",
            "Otp.child_count",
        ] {
            assert!(
                lib.lookup(name).is_some(),
                "registry must contain built-in '{}'",
                name
            );
        }
    }

    #[test]
    fn registry_entries_are_consistent() {
        let lib = StdLib::new();
        for op in lib.ops() {
            assert_eq!(
                format!("{}.{}", op.effect, op.op),
                op.name,
                "name must equal effect.op for '{}'",
                op.name
            );
            assert!(!op.signature.is_empty(), "'{}' needs a signature", op.name);
            assert!(
                op.signature.starts_with(op.op),
                "signature of '{}' must start with the op name",
                op.name
            );
            assert!(
                !op.description.is_empty(),
                "'{}' needs a description",
                op.name
            );
        }
    }

    #[test]
    fn lookup_reports_impl_sites() {
        let lib = StdLib::new();
        assert_eq!(
            lib.lookup("IO.print").unwrap().implemented_in,
            ImplSite::StandaloneVm
        );
        assert_eq!(
            lib.lookup("IO.read").unwrap().implemented_in,
            ImplSite::StandaloneVm
        );
        assert_eq!(
            lib.lookup("Timer.sleep").unwrap().implemented_in,
            ImplSite::RuntimeHost
        );
        assert_eq!(
            lib.lookup("Signal.wait").unwrap().implemented_in,
            ImplSite::RuntimeHost
        );
        assert_eq!(
            lib.lookup("LLM.ask").unwrap().implemented_in,
            ImplSite::RuntimeHost
        );
        assert_eq!(
            lib.lookup("Actor.link").unwrap().implemented_in,
            ImplSite::RuntimeHost
        );
        assert_eq!(
            lib.lookup("Actor.whereis").unwrap().implemented_in,
            ImplSite::RuntimeHost
        );
    }

    #[test]
    fn effects_lists_distinct_effects_in_order() {
        let lib = StdLib::new();
        assert_eq!(
            lib.effects(),
            vec!["IO", "Int", "Timer", "Signal", "LLM", "Actor", "Otp"]
        );
    }

    #[test]
    fn lookup_unknown_returns_none() {
        let lib = StdLib::new();
        assert!(lib.lookup("Net.send").is_none());
        assert!(lib.lookup("IO.nonexistent").is_none());
    }

    #[test]
    fn require_unknown_is_an_error() {
        let lib = StdLib::new();
        let err = lib.require("Net.send").unwrap_err();
        let msg = format!("{}", err);
        assert!(
            msg.contains("Net.send"),
            "error must name the operation: {}",
            msg
        );
    }

    #[test]
    fn docs_mention_every_registered_op() {
        let docs = stdlib_docs();
        let lib = StdLib::new();
        for op in lib.ops() {
            assert!(
                docs.contains(op.signature),
                "docs must include the signature of '{}'",
                op.name
            );
            assert!(
                docs.contains(op.description),
                "docs must include the description of '{}'",
                op.name
            );
        }
    }
}
