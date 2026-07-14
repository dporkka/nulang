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
            .ok_or_else(|| NuError::RuntimeError(format!("unknown built-in operation '{}'", name)))
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
            assert!(!op.description.is_empty(), "'{}' needs a description", op.name);
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
    }

    #[test]
    fn effects_lists_distinct_effects_in_order() {
        let lib = StdLib::new();
        assert_eq!(lib.effects(), vec!["IO", "Timer", "Signal", "LLM"]);
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
        assert!(msg.contains("Net.send"), "error must name the operation: {}", msg);
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
