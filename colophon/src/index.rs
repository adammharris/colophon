//! Index — where stable IDs and (later) the materialized graph live.
//!
//! The [`IndexStore`] is the single artifact that fuses three jobs: the
//! **authoritative** id↔path registry, and (to come) the **derived** resolution
//! cache and adjacency index. Only the registry is non-rebuildable; the derived
//! parts are a pure function of the documents and can always be regenerated.
//!
//! Keeping the store behind a trait is deliberate: a sidecar file, an in-memory
//! map, or a sync-backed store (diaryx's Durable Object) are all valid homes,
//! and a single central file would otherwise be a merge/contention hotspot.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::identity::Id;

/// Somewhere IDs (and eventually derived graph data) are persisted and queried.
pub trait IndexStore {
    /// Record that `id` names the document at `path`.
    fn register(&mut self, id: &Id, path: &Path);

    /// Resolve an ID to its current path.
    fn resolve(&self, id: &Id) -> Option<PathBuf>;

    /// The ID currently assigned to `path`, if any.
    fn id_for_path(&self, path: &Path) -> Option<Id>;

    /// Update the path an ID points at (e.g. after a move/rename).
    fn set_path(&mut self, id: &Id, new_path: &Path);

    /// Forget an ID (e.g. after a delete, absent tombstoning).
    fn unregister(&mut self, id: &Id);
}

/// No index — identity-off workspaces. Registers nothing, resolves nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoIndex;

impl IndexStore for NoIndex {
    fn register(&mut self, _id: &Id, _path: &Path) {}
    fn resolve(&self, _id: &Id) -> Option<PathBuf> {
        None
    }
    fn id_for_path(&self, _path: &Path) -> Option<Id> {
        None
    }
    fn set_path(&mut self, _id: &Id, _new_path: &Path) {}
    fn unregister(&mut self, _id: &Id) {}
}

/// A simple in-memory registry — the default backing store, and the model a
/// persistent [`IndexStore`] serializes to/from.
#[derive(Debug, Clone, Default)]
pub struct InMemoryIndex {
    forward: HashMap<Id, PathBuf>,
    reverse: HashMap<PathBuf, Id>,
}

impl InMemoryIndex {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of registered IDs.
    pub fn len(&self) -> usize {
        self.forward.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.forward.is_empty()
    }
}

impl IndexStore for InMemoryIndex {
    fn register(&mut self, id: &Id, path: &Path) {
        self.forward.insert(id.clone(), path.to_path_buf());
        self.reverse.insert(path.to_path_buf(), id.clone());
    }

    fn resolve(&self, id: &Id) -> Option<PathBuf> {
        self.forward.get(id).cloned()
    }

    fn id_for_path(&self, path: &Path) -> Option<Id> {
        self.reverse.get(path).cloned()
    }

    fn set_path(&mut self, id: &Id, new_path: &Path) {
        if let Some(old) = self.forward.insert(id.clone(), new_path.to_path_buf()) {
            self.reverse.remove(&old);
        }
        self.reverse.insert(new_path.to_path_buf(), id.clone());
    }

    fn unregister(&mut self, id: &Id) {
        if let Some(path) = self.forward.remove(id) {
            self.reverse.remove(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_and_resolves_both_directions() {
        let mut ix = InMemoryIndex::new();
        let id = Id("ajp7eq".into());
        ix.register(&id, Path::new("notes/a.md"));
        assert_eq!(ix.resolve(&id), Some(PathBuf::from("notes/a.md")));
        assert_eq!(ix.id_for_path(Path::new("notes/a.md")), Some(id.clone()));
        assert_eq!(ix.len(), 1);
    }

    #[test]
    fn move_updates_path_and_clears_stale_reverse() {
        let mut ix = InMemoryIndex::new();
        let id = Id("ajp7eq".into());
        ix.register(&id, Path::new("a.md"));
        ix.set_path(&id, Path::new("moved/a.md"));
        assert_eq!(ix.resolve(&id), Some(PathBuf::from("moved/a.md")));
        assert_eq!(ix.id_for_path(Path::new("a.md")), None);
        assert_eq!(ix.id_for_path(Path::new("moved/a.md")), Some(id));
    }

    #[test]
    fn unregister_removes_both_directions() {
        let mut ix = InMemoryIndex::new();
        let id = Id("x".into());
        ix.register(&id, Path::new("a.md"));
        ix.unregister(&id);
        assert!(ix.is_empty());
        assert_eq!(ix.id_for_path(Path::new("a.md")), None);
    }
}
