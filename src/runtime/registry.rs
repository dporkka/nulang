//! Local actor name registry: register/unregister/whereis/registered.
//!
//! Per-node local registry, similar to Erlang's local process registry.
//! Cluster-wide naming is handled by virtual actors.

use std::collections::HashMap;
use std::sync::RwLock;

/// Errors that can occur during registry operations.
#[derive(Debug, Clone, PartialEq)]
pub enum RegisterError {
    NameAlreadyRegistered(String),
    InvalidName(String),
    ActorNotFound(u64),
}

impl std::fmt::Display for RegisterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegisterError::NameAlreadyRegistered(name) => {
                write!(f, "Name '{}' is already registered", name)
            }
            RegisterError::InvalidName(name) => {
                write!(f, "Invalid actor name: '{}'", name)
            }
            RegisterError::ActorNotFound(id) => {
                write!(f, "Actor {} not found", id)
            }
        }
    }
}

impl std::error::Error for RegisterError {}

/// Local actor name registry.
///
/// Maps human-readable names to actor IDs. Each node has its own registry.
/// Names are unique within a node. Registration fails if the name is already
/// in use.
pub struct ActorRegistry {
    names: RwLock<HashMap<String, u64>>,
    reverse: RwLock<HashMap<u64, String>>,
}

impl ActorRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        ActorRegistry {
            names: RwLock::new(HashMap::new()),
            reverse: RwLock::new(HashMap::new()),
        }
    }

    /// Register a name for an actor.
    pub fn register(&self, name: &str, actor_id: u64) -> Result<(), RegisterError> {
        Self::validate_name(name)?;

        let mut names = self.names.write().map_err(|_| {
            RegisterError::InvalidName(name.to_string())
        })?;

        if names.contains_key(name) {
            return Err(RegisterError::NameAlreadyRegistered(name.to_string()));
        }

        names.insert(name.to_string(), actor_id);

        // Update reverse mapping
        if let Ok(mut reverse) = self.reverse.write() {
            reverse.insert(actor_id, name.to_string());
        }

        Ok(())
    }

    /// Unregister a name.
    pub fn unregister(&self, name: &str) -> Result<(), RegisterError> {
        let mut names = self.names.write().map_err(|_| {
            RegisterError::InvalidName(name.to_string())
        })?;

        let actor_id = names.remove(name).ok_or_else(|| {
            RegisterError::InvalidName(name.to_string())
        })?;

        if let Ok(mut reverse) = self.reverse.write() {
            reverse.remove(&actor_id);
        }

        Ok(())
    }

    /// Look up an actor ID by name.
    pub fn whereis(&self, name: &str) -> Option<u64> {
        let names = self.names.read().ok()?;
        names.get(name).copied()
    }

    /// List all registered names.
    pub fn registered(&self) -> Vec<String> {
        let names = match self.names.read() {
            Ok(n) => n,
            Err(_) => return Vec::new(),
        };
        names.keys().cloned().collect()
    }

    /// Remove all names for a given actor (called on actor exit).
    pub fn unregister_by_actor(&self, actor_id: u64) -> Vec<String> {
        let mut removed = Vec::new();

        // Get the name from reverse mapping
        if let Ok(reverse) = self.reverse.read() {
            if let Some(name) = reverse.get(&actor_id) {
                removed.push(name.clone());
            }
        }

        // Remove from both mappings
        if let Ok(mut names) = self.names.write() {
            for name in &removed {
                names.remove(name);
            }
        }

        if let Ok(mut reverse) = self.reverse.write() {
            reverse.remove(&actor_id);
        }

        removed
    }

    /// Check if a name is registered.
    pub fn is_registered(&self, name: &str) -> bool {
        let names = match self.names.read() {
            Ok(n) => n,
            Err(_) => return false,
        };
        names.contains_key(name)
    }

    fn validate_name(name: &str) -> Result<(), RegisterError> {
        if name.is_empty() {
            return Err(RegisterError::InvalidName("(empty)".to_string()));
        }
        if name.chars().any(|c| c.is_whitespace()) {
            return Err(RegisterError::InvalidName(name.to_string()));
        }
        if name.chars().next().unwrap().is_ascii_digit() {
            return Err(RegisterError::InvalidName(name.to_string()));
        }
        if !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Err(RegisterError::InvalidName(name.to_string()));
        }
        Ok(())
    }
}

impl Default for ActorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_whereis() {
        let reg = ActorRegistry::new();
        reg.register("my_actor", 42).unwrap();
        assert_eq!(reg.whereis("my_actor"), Some(42));
    }

    #[test]
    fn test_unregister() {
        let reg = ActorRegistry::new();
        reg.register("temp", 42).unwrap();
        reg.unregister("temp").unwrap();
        assert_eq!(reg.whereis("temp"), None);
    }

    #[test]
    fn test_duplicate_name_fails() {
        let reg = ActorRegistry::new();
        reg.register("shared", 1).unwrap();
        assert!(reg.register("shared", 2).is_err());
    }

    #[test]
    fn test_invalid_names() {
        let reg = ActorRegistry::new();
        assert!(reg.register("", 1).is_err());
        assert!(reg.register("has space", 1).is_err());
        assert!(reg.register("1digit", 1).is_err());
    }

    #[test]
    fn test_unregister_by_actor() {
        let reg = ActorRegistry::new();
        reg.register("dies_soon", 42).unwrap();
        let removed = reg.unregister_by_actor(42);
        assert!(removed.contains(&"dies_soon".to_string()));
        assert_eq!(reg.whereis("dies_soon"), None);
    }

    #[test]
    fn test_registered_list() {
        let reg = ActorRegistry::new();
        reg.register("first", 1).unwrap();
        reg.register("second", 2).unwrap();
        let names = reg.registered();
        assert_eq!(names.len(), 2);
    }
}