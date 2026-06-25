//! Process groups (pg): named groups of actors, similar to Erlang's pg module.
//!
//! Actors can join named groups, leave them, and other actors can query
//! group membership. When an actor exits, it is automatically removed
//! from all groups.

use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

/// Errors that can occur during process group operations.
#[derive(Debug, Clone, PartialEq)]
pub enum PgError {
    /// The group name is invalid (empty or contains whitespace).
    InvalidGroup(String),
}

impl std::fmt::Display for PgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PgError::InvalidGroup(name) => write!(f, "Invalid group name: '{}'", name),
        }
    }
}

impl std::error::Error for PgError {}

/// Process groups: named collections of actor IDs.
///
/// Similar to Erlang's `pg` module, this provides a way to organize actors
/// into named groups for discovery and broadcast purposes.
///
/// # Example
/// ```
/// use nulang_impl::runtime::ProcessGroups;
///
/// let groups = ProcessGroups::new();
/// groups.join("workers", 42).unwrap();
/// assert!(groups.is_member("workers", 42));
/// ```
pub struct ProcessGroups {
    /// group_name -> set of actor_ids
    groups: RwLock<HashMap<String, HashSet<u64>>>,
}

impl ProcessGroups {
    /// Creates a new, empty process groups container.
    pub fn new() -> Self {
        ProcessGroups {
            groups: RwLock::new(HashMap::new()),
        }
    }

    /// Join a group. If the group doesn't exist, it is created.
    ///
    /// Returns an error if the group name is invalid.
    pub fn join(&self, group: &str, actor_id: u64) -> Result<(), PgError> {
        if group.is_empty() || group.chars().any(|c| c.is_whitespace()) {
            return Err(PgError::InvalidGroup(group.to_string()));
        }

        let mut groups = self.groups.write().map_err(|_| {
            PgError::InvalidGroup(group.to_string())
        })?;

        groups.entry(group.to_string())
            .or_insert_with(HashSet::new)
            .insert(actor_id);

        Ok(())
    }

    /// Leave a group. Returns true if the actor was in the group.
    ///
    /// If the group becomes empty after leaving, it is removed.
    pub fn leave(&self, group: &str, actor_id: u64) -> bool {
        let mut groups = match self.groups.write() {
            Ok(g) => g,
            Err(_) => return false,
        };

        let was_member = if let Some(members) = groups.get_mut(group) {
            members.remove(&actor_id)
        } else {
            false
        };

        // Clean up empty groups
        if let Some(members) = groups.get(group) {
            if members.is_empty() {
                groups.remove(group);
            }
        }

        was_member
    }

    /// Remove an actor from all groups it belongs to.
    ///
    /// Returns the list of group names the actor was removed from.
    pub fn leave_all(&self, actor_id: u64) -> Vec<String> {
        let mut removed = Vec::new();

        let mut groups = match self.groups.write() {
            Ok(g) => g,
            Err(_) => return removed,
        };

        let mut empty_groups = Vec::new();

        for (group_name, members) in groups.iter_mut() {
            if members.remove(&actor_id) {
                removed.push(group_name.clone());
                if members.is_empty() {
                    empty_groups.push(group_name.clone());
                }
            }
        }

        for group_name in empty_groups {
            groups.remove(&group_name);
        }

        removed
    }

    /// Get the members of a group.
    ///
    /// Returns an empty vector if the group doesn't exist.
    pub fn members(&self, group: &str) -> Vec<u64> {
        let groups = match self.groups.read() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };

        groups.get(group)
            .map(|members| members.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Check if an actor is a member of a group.
    pub fn is_member(&self, group: &str, actor_id: u64) -> bool {
        let groups = match self.groups.read() {
            Ok(g) => g,
            Err(_) => return false,
        };

        groups.get(group)
            .map(|members| members.contains(&actor_id))
            .unwrap_or(false)
    }

    /// Get the number of members in a group.
    pub fn member_count(&self, group: &str) -> usize {
        let groups = match self.groups.read() {
            Ok(g) => g,
            Err(_) => return 0,
        };

        groups.get(group)
            .map(|members| members.len())
            .unwrap_or(0)
    }

    /// Get a list of all non-empty group names.
    pub fn which_groups(&self) -> Vec<String> {
        let groups = match self.groups.read() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };

        groups.keys().cloned().collect()
    }
}

impl Default for ProcessGroups {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_is_empty() {
        let pg = ProcessGroups::new();
        assert!(pg.which_groups().is_empty());
    }

    #[test]
    fn test_join_and_members() {
        let pg = ProcessGroups::new();
        pg.join("workers", 42).unwrap();
        pg.join("workers", 43).unwrap();

        let members = pg.members("workers");
        assert_eq!(members.len(), 2);
        assert!(members.contains(&42));
        assert!(members.contains(&43));
    }

    #[test]
    fn test_leave() {
        let pg = ProcessGroups::new();
        pg.join("chat", 42).unwrap();
        assert!(pg.leave("chat", 42));
        assert!(pg.members("chat").is_empty());
    }

    #[test]
    fn test_leave_all() {
        let pg = ProcessGroups::new();
        pg.join("group_a", 42).unwrap();
        pg.join("group_b", 42).unwrap();
        pg.join("group_a", 43).unwrap();

        let removed_from = pg.leave_all(42);
        assert_eq!(removed_from.len(), 2);

        assert!(pg.is_member("group_a", 43));
        assert!(!pg.is_member("group_a", 42));
        assert!(!pg.is_member("group_b", 42));
    }

    #[test]
    fn test_idempotent_join() {
        let pg = ProcessGroups::new();
        pg.join("singleton", 42).unwrap();
        pg.join("singleton", 42).unwrap();

        assert_eq!(pg.member_count("singleton"), 1);
    }

    #[test]
    fn test_empty_group_cleanup() {
        let pg = ProcessGroups::new();
        pg.join("temp", 42).unwrap();
        pg.leave("temp", 42);

        assert!(pg.which_groups().is_empty());
    }

    #[test]
    fn test_invalid_group_name() {
        let pg = ProcessGroups::new();
        assert!(pg.join("", 42).is_err());
        assert!(pg.join("has space", 42).is_err());
    }

    #[test]
    fn test_is_member() {
        let pg = ProcessGroups::new();
        assert!(!pg.is_member("workers", 42));
        pg.join("workers", 42).unwrap();
        assert!(pg.is_member("workers", 42));
    }

    #[test]
    fn test_member_count() {
        let pg = ProcessGroups::new();
        assert_eq!(pg.member_count("workers"), 0);
        pg.join("workers", 1).unwrap();
        pg.join("workers", 2).unwrap();
        pg.join("workers", 3).unwrap();
        assert_eq!(pg.member_count("workers"), 3);
    }

    #[test]
    fn test_leave_nonexistent_group() {
        let pg = ProcessGroups::new();
        assert!(!pg.leave("nonexistent", 42));
    }

    #[test]
    fn test_leave_all_not_in_any_group() {
        let pg = ProcessGroups::new();
        let removed = pg.leave_all(42);
        assert!(removed.is_empty());
    }
}