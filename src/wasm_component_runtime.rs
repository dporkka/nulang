use crate::types::{NuError, NuResult, Span};

#[cfg(feature = "wasm-backend")]
use wasmtime::component::*;
#[cfg(feature = "wasm-backend")]
use wasmtime::*;

#[cfg(feature = "wasm-backend")]
bindgen!({
    world: "actor",
    path: "wit/actor.wit",
});

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

#[cfg(feature = "wasm-backend")]
pub struct HostState {}

#[cfg(feature = "wasm-backend")]
pub struct ComponentRuntime {
    _engine: Engine,
    component: Component,
    config: Config,
}

#[cfg(feature = "wasm-backend")]
impl ComponentRuntime {
    pub fn new(wasm_bytes: &[u8]) -> NuResult<Self> {
        let config = component_config();
        let engine = Engine::new(&config)
            .map_err(|e| NuError::VMError { msg: format!("wasmtime engine: {}", e), span: Span::default() })?;
        let component = Component::new(&engine, wasm_bytes)
            .map_err(|e| NuError::VMError { msg: format!("wasmtime component: {}", e), span: Span::default() })?;

        let config = component_config();
        let engine = Engine::new(&config)
            .map_err(|e| NuError::VMError { msg: format!("wasmtime engine: {}", e), span: Span::default() })?;
        Ok(ComponentRuntime {
            _engine: engine,
            component,
            config,
        })
    }

    pub fn init(&self) -> NuResult<i64> {
        let engine = Engine::new(&self.config)
            .map_err(|e| NuError::VMError { msg: format!("wasmtime engine: {}", e), span: Span::default() })?;
        let mut store = Store::new(&engine, HostState {});
        let linker = wasmtime::component::Linker::<HostState>::new(&engine);

        let actor = Actor::instantiate(&mut store, &self.component, &linker)
            .map_err(|e| NuError::VMError { msg: format!("wasmtime instantiate: {}", e), span: Span::default() })?;

        actor
            .call_init(&mut store)
            .map_err(|e| NuError::VMError { msg: format!("wasmtime call_init: {}", e), span: Span::default() })
    }

    pub fn handle_message(&self, msg: &[u8]) -> NuResult<i64> {
        let engine = Engine::new(&self.config)
            .map_err(|e| NuError::VMError { msg: format!("wasmtime engine: {}", e), span: Span::default() })?;
        let mut store = Store::new(&engine, HostState {});
        let linker = wasmtime::component::Linker::<HostState>::new(&engine);

        let actor = Actor::instantiate(&mut store, &self.component, &linker)
            .map_err(|e| NuError::VMError { msg: format!("wasmtime instantiate: {}", e), span: Span::default() })?;

        actor
            .call_handle_message(&mut store, msg)
            .map_err(|e| NuError::VMError { msg: format!("wasmtime call_handle_message: {}", e), span: Span::default() })
    }

    pub fn checkpoint(&self) -> NuResult<Vec<u8>> {
        let engine = Engine::new(&self.config)
            .map_err(|e| NuError::VMError { msg: format!("wasmtime engine: {}", e), span: Span::default() })?;
        let mut store = Store::new(&engine, HostState {});
        let linker = wasmtime::component::Linker::<HostState>::new(&engine);

        let actor = Actor::instantiate(&mut store, &self.component, &linker)
            .map_err(|e| NuError::VMError { msg: format!("wasmtime instantiate: {}", e), span: Span::default() })?;

        actor
            .call_checkpoint(&mut store)
            .map_err(|e| NuError::VMError { msg: format!("wasmtime call_checkpoint: {}", e), span: Span::default() })
    }
}
