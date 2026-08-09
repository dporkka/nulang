use crate::types::{NuError, NuResult, Span};

#[cfg(feature = "wasm-backend")]
use wasmtime::component::*;
#[cfg(feature = "wasm-backend")]
use wasmtime::*;

#[cfg(feature = "wasm-backend")]
mod bindings {
    wasmtime::component::bindgen!({
        world: "actor",
        path: "wit/actor.wit",
    });
}

#[cfg(feature = "wasm-backend")]
use bindings::Actor;
#[cfg(feature = "wasm-backend")]
use bindings::host::Host as HostTrait;

#[cfg(feature = "wasm-backend")]
pub fn component_config() -> Config {
    let mut config = Config::new();
    config.wasm_component_model(true);
    config.memory_reservation(4 << 30);
    config.memory_guard_size(128 << 20);
    config.cranelift_opt_level(OptLevel::Speed);
    config.wasm_simd(true);
    config
}

/// Capability controls for the WASM component host.
///
/// Each flag determines whether a given host function is made available to
/// the guest component. If a flag is `false` and the component imports the
/// corresponding function, instantiation will fail at the linker level.
#[cfg(feature = "wasm-backend")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub allow_log: bool,
    pub allow_clock: bool,
    pub allow_random: bool,
}

#[cfg(feature = "wasm-backend")]
impl Default for Capabilities {
    fn default() -> Self {
        Capabilities {
            allow_log: false,
            allow_clock: false,
            allow_random: false,
        }
    }
}

#[cfg(feature = "wasm-backend")]
pub struct HostState {
    caps: Capabilities,
    log_messages: Vec<String>,
}

#[cfg(feature = "wasm-backend")]
impl HostTrait for HostState {
    fn log(&mut self, msg: String) {
        if self.caps.allow_log {
            self.log_messages.push(msg);
        }
    }

    fn clock_now(&mut self) -> u64 {
        if self.caps.allow_clock {
            0
        } else {
            panic!("clock capability denied")
        }
    }

    fn random_u64(&mut self) -> u64 {
        if self.caps.allow_random {
            0
        } else {
            panic!("random capability denied")
        }
    }
}

#[cfg(feature = "wasm-backend")]
pub struct ComponentRuntime {
    _engine: Engine,
    component: Component,
    config: Config,
    caps: Capabilities,
}

#[cfg(feature = "wasm-backend")]
impl ComponentRuntime {
    pub fn new(wasm_bytes: &[u8]) -> NuResult<Self> {
        Self::new_with_caps(wasm_bytes, Capabilities::default())
    }

    pub fn new_with_caps(wasm_bytes: &[u8], caps: Capabilities) -> NuResult<Self> {
        let config = component_config();
        let engine = Engine::new(&config).map_err(|e| NuError::VMError {
            msg: format!("wasmtime engine: {}", e),
            span: Span::default(),
        })?;
        let component = Component::new(&engine, wasm_bytes).map_err(|e| NuError::VMError {
            msg: format!("wasmtime component: {}", e),
            span: Span::default(),
        })?;
        Ok(ComponentRuntime {
            _engine: engine,
            component,
            config,
            caps,
        })
    }

    fn build_linker(&self, engine: &Engine) -> NuResult<wasmtime::component::Linker<HostState>> {
        let mut linker = wasmtime::component::Linker::<HostState>::new(engine);
        
        // Add the host instance manually with the correct name
        if self.caps.allow_log || self.caps.allow_clock || self.caps.allow_random {
            let mut inst = linker
                .instance("nulang:runtime/host")
                .map_err(|e| NuError::VMError {
                    msg: format!("wasmtime linker: {}", e),
                    span: Span::default(),
                })?;
            if self.caps.allow_log {
                inst.func_wrap(
                    "log",
                    move |mut caller: wasmtime::StoreContextMut<'_, HostState>,
                          (msg,): (String,)| {
                        caller.data_mut().log_messages.push(msg);
                        Ok(())
                    },
                )
                .map_err(|e| NuError::VMError {
                    msg: format!("wasmtime linker: {}", e),
                    span: Span::default(),
                })?;
            }
            if self.caps.allow_clock {
                inst.func_wrap(
                    "clock-now",
                    move |mut caller: wasmtime::StoreContextMut<'_, HostState>, _: ()| {
                        let r = caller.data_mut().clock_now();
                        Ok((r,))
                    },
                )
                .map_err(|e| NuError::VMError {
                    msg: format!("wasmtime linker: {}", e),
                    span: Span::default(),
                })?;
            }
            if self.caps.allow_random {
                inst.func_wrap(
                    "random-u64",
                    move |mut caller: wasmtime::StoreContextMut<'_, HostState>, _: ()| {
                        let r = caller.data_mut().random_u64();
                        Ok((r,))
                    },
                )
                .map_err(|e| NuError::VMError {
                    msg: format!("wasmtime linker: {}", e),
                    span: Span::default(),
                })?;
            }
        }
        Ok(linker)
    }

    pub fn init(&self) -> NuResult<i64> {
        let engine = Engine::new(&self.config).map_err(|e| NuError::VMError {
            msg: format!("wasmtime engine: {}", e),
            span: Span::default(),
        })?;
        let mut store = Store::new(
            &engine,
            HostState {
                caps: self.caps,
                log_messages: Vec::new(),
            },
        );
        let linker = self.build_linker(&engine)?;

        let actor = Actor::instantiate(&mut store, &self.component, &linker).map_err(|e| {
            NuError::VMError {
                msg: format!("wasmtime instantiate: {}", e),
                span: Span::default(),
            }
        })?;

        actor.call_init(&mut store).map_err(|e| NuError::VMError {
            msg: format!("wasmtime call_init: {}", e),
            span: Span::default(),
        })
    }

    pub fn handle_message(&self, msg: &[u8]) -> NuResult<i64> {
        let engine = Engine::new(&self.config).map_err(|e| NuError::VMError {
            msg: format!("wasmtime engine: {}", e),
            span: Span::default(),
        })?;
        let mut store = Store::new(
            &engine,
            HostState {
                caps: self.caps,
                log_messages: Vec::new(),
            },
        );
        let linker = self.build_linker(&engine)?;

        let actor = Actor::instantiate(&mut store, &self.component, &linker).map_err(|e| {
            NuError::VMError {
                msg: format!("wasmtime instantiate: {}", e),
                span: Span::default(),
            }
        })?;

        actor
            .call_handle_message(&mut store, msg)
            .map_err(|e| NuError::VMError {
                msg: format!("wasmtime call_handle_message: {}", e),
                span: Span::default(),
            })
    }

    pub fn checkpoint(&self) -> NuResult<Vec<u8>> {
        let engine = Engine::new(&self.config).map_err(|e| NuError::VMError {
            msg: format!("wasmtime engine: {}", e),
            span: Span::default(),
        })?;
        let mut store = Store::new(
            &engine,
            HostState {
                caps: self.caps,
                log_messages: Vec::new(),
            },
        );
        let linker = self.build_linker(&engine)?;

        let actor = Actor::instantiate(&mut store, &self.component, &linker).map_err(|e| {
            NuError::VMError {
                msg: format!("wasmtime instantiate: {}", e),
                span: Span::default(),
            }
        })?;

        actor
            .call_checkpoint(&mut store)
            .map_err(|e| NuError::VMError {
                msg: format!("wasmtime call_checkpoint: {}", e),
                span: Span::default(),
            })
    }
}

#[cfg(test)]
#[cfg(feature = "wasm-backend")]
mod tests {
    use super::*;
    


    /// Minimal WAT component that imports `log` and exports `init`.
    const LOG_IMPORT_WAT: &str = r#"
        (component
            (import "nulang:runtime/host" (instance $host
                (export "log" (func (param "msg" string)))
            ))
        )
    "#;


    #[test]
    fn test_component_capability_gate_denies_log() {
        let wasm = wat::parse_str(LOG_IMPORT_WAT).expect("parse WAT");
        let rt = ComponentRuntime::new_with_caps(&wasm, Capabilities {
            allow_log: false,
            allow_clock: false,
            allow_random: false,
        }).expect("new_with_caps");

        // Direct instantiation with the linker should fail because the host
        // interface is not added when allow_log is false.
        let engine = wasmtime::Engine::new(&rt.config).expect("engine");
        let mut store = wasmtime::Store::new(
            &engine,
            HostState {
                caps: rt.caps,
                log_messages: Vec::new(),
            },
        );
        let linker = rt.build_linker(&engine).expect("linker");
        let component = wasmtime::component::Component::new(&engine, &wasm).expect("component");
        let err = linker
            .instantiate(&mut store, &component)
            .expect_err("should fail to instantiate without log capability");
        assert!(
            err.to_string().contains("host"),
            "error should mention missing host import: {}",
            err
        );
    }

    #[test]
    fn test_component_capability_gate_allows_log() {
        let wasm = wat::parse_str(LOG_IMPORT_WAT).expect("parse WAT");
        let rt = ComponentRuntime::new_with_caps(&wasm, Capabilities {
            allow_log: true,
            allow_clock: false,
            allow_random: false,
        }).expect("new_with_caps");

        // Direct instantiation with the linker should succeed because the host
        // interface is added when allow_log is true.
        let engine = wasmtime::Engine::new(&rt.config).expect("engine");
        let mut store = wasmtime::Store::new(
            &engine,
            HostState {
                caps: rt.caps,
                log_messages: Vec::new(),
            },
        );
        let linker = rt.build_linker(&engine).expect("linker");
        let component = wasmtime::component::Component::new(&engine, &wasm).expect("component");
        let _instance = linker
            .instantiate(&mut store, &component)
            .expect("should succeed with log capability");
    }
}
