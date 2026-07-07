//! Index — where stable IDs and (later) the materialized graph live.
//!
//! The [`IndexStore`] is the single artifact that fuses two natures (DESIGN
//! §5): the **authoritative** id↔path registry — not rebuildable from the
//! documents — and (to come) the **derived** resolution cache and adjacency
//! index, which are. Keeping the store behind a trait is deliberate: a sidecar
//! file, an in-memory map, or a sync-backed store are all valid homes.
//!
//! ## Tombstones — IDs are forever
//!
//! DESIGN's open question #1 ("does the registry ever need to survive without
//! its documents?") is answered **yes, minimally**: deleting a document leaves
//! a *tombstone* — the ID stops resolving but is never forgotten, so it can
//! never be reminted to mean something else. A dangling `colophon:` reference
//! then stays *diagnosable* (validation can say "that document was deleted")
//! instead of becoming a silent re-resolution hazard.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use crate::document::{Document, MetaCarrier, whole_file_format};
use crate::edit::MetaEditor;
use crate::error::Result;
use crate::identity::Id;
use crate::meta::{Mapping, Value};

/// Somewhere IDs (and eventually derived graph data) are persisted and queried.
pub trait IndexStore {
    /// Record that `id` names the document at `path`.
    fn register(&mut self, id: &Id, path: &Path);

    /// Resolve an ID to its current path. `None` for unknown *and* tombstoned
    /// IDs — use [`is_known`](IndexStore::is_known) to tell them apart.
    fn resolve(&self, id: &Id) -> Option<PathBuf>;

    /// The ID currently assigned to `path`, if any.
    fn id_for_path(&self, path: &Path) -> Option<Id>;

    /// Update the path an ID points at (e.g. after a move/rename).
    fn set_path(&mut self, id: &Id, new_path: &Path);

    /// Retire an ID (e.g. after a delete). A store with tombstones keeps the
    /// ID on record so it is never reissued; a plain store may forget it.
    fn unregister(&mut self, id: &Id);

    /// Whether `id` has *ever* been issued — live or tombstoned. This is the
    /// mint-with-rejection predicate: a fresh ID must be `!is_known`.
    fn is_known(&self, id: &Id) -> bool {
        self.resolve(id).is_some()
    }
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

/// A simple in-memory registry — for tests and ephemeral workspaces. No
/// tombstones: an unregistered ID is forgotten entirely.
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

/// The persistent registry: a snapshot with tombstones, living **under the
/// `registry` key of a workspace document** — the document the root's
/// registry-pointer relation targets.
///
/// The host document can be either shape (`MetaCarrier`): a bare config file
/// (`registry.yaml`, `registry.figl`, …) whose whole content is metadata, or a
/// prose document (`registry.md`) whose fenced frontmatter carries the records.
/// Writes splice only the `registry` value back through the carrier-aware
/// editor, so the host's other keys (`title`, `part_of` — the self-description
/// that makes the registry a first-class node of the tree), its comments
/// outside the records, its body, and its fence style all survive.
///
/// The rendered records are one per line (in YAML hosts), sorted by ID; a live
/// record is `id: path`, a tombstone is `id: null` (DESIGN §5's diff-friendly
/// shape). This type is pure — text in ([`FileIndex::parse`]), text out
/// ([`FileIndex::render`]) — so any storage backend can host it; the caller
/// owns the I/O and can consult [`is_dirty`](FileIndex::is_dirty) to skip
/// no-op writes.
#[derive(Debug, Clone)]
pub struct FileIndex {
    live: InMemoryIndex,
    tombstones: BTreeSet<Id>,
    /// The host document's full current text and carrier — what `render`
    /// splices the records back into.
    host_text: String,
    carrier: MetaCarrier,
    /// The record state as currently written in `host_text` — `render` applies
    /// only the per-record diff against this, as scalar upserts (whole-mapping
    /// splices cannot round-trip through every carrier; scalars can).
    persisted: BTreeMap<Id, Option<String>>,
    /// Whether `host_text` already has a `registry` key. When it does not, the
    /// first render inserts the whole mapping at once — that is what gets the
    /// block (one-record-per-line) layout on bare hosts; per-record creation
    /// would make fig auto-create a flow map.
    has_registry_key: bool,
    dirty: bool,
}

impl FileIndex {
    /// An empty registry hosted by an (empty) bare config document in `format`.
    pub fn new(format: fig::Format) -> Self {
        Self {
            live: InMemoryIndex::new(),
            tombstones: BTreeSet::new(),
            host_text: String::new(),
            carrier: MetaCarrier::WholeFile(format),
            persisted: BTreeMap::new(),
            has_registry_key: false,
            dirty: false,
        }
    }

    /// Parse the registry out of its host document. `path` picks the carrier
    /// (a config extension means the whole file is metadata; anything else is
    /// searched for a fenced block); the records are read from the metadata's
    /// `registry` key. A host with no `registry` key is an empty registry —
    /// the rest of its metadata is left alone.
    pub fn parse(path: &Path, text: &str) -> Result<Self> {
        let doc = Document::parse(path, text)?;
        let carrier = doc.carrier.unwrap_or_else(|| {
            // No metadata yet: default by extension, else fresh YAML frontmatter.
            whole_file_format(path)
                .map(MetaCarrier::WholeFile)
                .unwrap_or(MetaCarrier::Fenced(fig::EmbedType::FrontmatterYaml))
        });
        let mut index = Self {
            live: InMemoryIndex::new(),
            tombstones: BTreeSet::new(),
            host_text: text.to_string(),
            carrier,
            persisted: BTreeMap::new(),
            has_registry_key: doc.meta.get("registry").is_some(),
            dirty: false,
        };
        if let Some(registry) = doc.meta.get("registry").and_then(Value::as_mapping) {
            for (id, value) in registry {
                let id = Id(id.clone());
                match value {
                    Value::Null => {
                        index.persisted.insert(id.clone(), None);
                        index.tombstones.insert(id);
                    }
                    Value::String(path) => {
                        index.persisted.insert(id.clone(), Some(path.clone()));
                        index.live.register(&id, Path::new(path));
                    }
                    _ => {
                        return Err(crate::error::Error::Structure(format!(
                            "registry entry `{id}` must be a path or null (tombstone)"
                        )));
                    }
                }
            }
        }
        Ok(index)
    }

    /// Render the host document with the current records applied to its
    /// `registry` key. Each changed record is a *scalar* upsert
    /// (`registry.<id> = path` / `null`), so everything else in the host —
    /// title, part_of, comments, body, fences, existing record lines — is
    /// untouched, whatever the carrier. Records never reorder; new ones land
    /// in ID order.
    pub fn render(&mut self) -> Result<String> {
        let mut current: BTreeMap<Id, Option<String>> = BTreeMap::new();
        for id in &self.tombstones {
            current.insert(id.clone(), None);
        }
        for (id, path) in &self.live.forward {
            current.insert(id.clone(), Some(path.to_string_lossy().into_owned()));
        }
        if current == self.persisted {
            return Ok(self.host_text.clone());
        }

        // First materialization of the `registry` key.
        if !self.has_registry_key {
            let mut registry = Mapping::new();
            for (id, value) in &current {
                registry.insert(
                    id.0.clone(),
                    value.clone().map(Value::String).unwrap_or(Value::Null),
                );
            }
            let rendered = match self.carrier {
                // Bare host: rebuild the whole config document (its metadata
                // plus the new registry mapping) through `serialize_mapping`,
                // whose block layout gives one record per line. This is the
                // one write that does not go through the comment-preserving
                // editor — a fig value splice renders short maps in flow
                // style, which would freeze the registry inline forever.
                // Bootstrap hosts are machine-generated, so nothing of note
                // is lost; afterwards every write is a preserving upsert.
                MetaCarrier::WholeFile(format) => {
                    let mut top = crate::meta::parse_mapping(&self.host_text, format)?;
                    top.insert("registry".into(), Value::Mapping(registry));
                    crate::meta::serialize_mapping(&top, format)?
                }
                // Fenced host: a block map cannot be spliced into the fence
                // (fig embed limitation), so records land per-key — fig
                // auto-creates the chain as a flow map. Valid, just inline.
                MetaCarrier::Fenced(_) => {
                    let mut editor = MetaEditor::open_or_init(&self.host_text, Some(self.carrier))?;
                    for (id, value) in &current {
                        let fig_value =
                            value.clone().map(fig::Value::Str).unwrap_or(fig::Value::Null);
                        editor.set_value(
                            &[fig::Segment::Key("registry"), fig::Segment::Key(id.as_str())],
                            fig_value,
                        )?;
                    }
                    editor.render()?
                }
            };
            self.host_text = rendered.clone();
            self.persisted = current;
            self.has_registry_key = true;
            return Ok(rendered);
        }

        // Steady state: per-record comment-preserving upserts of the diff.
        let mut editor = MetaEditor::open_or_init(&self.host_text, Some(self.carrier))?;
        for (id, value) in &current {
            if self.persisted.get(id) == Some(value) {
                continue;
            }
            let fig_value = value.clone().map(fig::Value::Str).unwrap_or(fig::Value::Null);
            editor.set_value(
                &[fig::Segment::Key("registry"), fig::Segment::Key(id.as_str())],
                fig_value,
            )?;
        }
        let rendered = editor.render()?;
        self.host_text = rendered.clone();
        self.persisted = current;
        Ok(rendered)
    }

    /// Whether the registry changed since it was parsed/created (i.e. needs a
    /// write). Cleared by [`mark_clean`](FileIndex::mark_clean).
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Mark the registry as persisted.
    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    /// The number of live (resolving) IDs.
    pub fn len(&self) -> usize {
        self.live.len()
    }

    /// Whether the registry has no live IDs.
    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    /// Whether `id` is retired: known but no longer resolving.
    pub fn is_tombstoned(&self, id: &Id) -> bool {
        self.tombstones.contains(id)
    }

    /// Iterate live records as `(id, path)`, sorted by ID.
    pub fn iter(&self) -> impl Iterator<Item = (&Id, &PathBuf)> {
        let mut live: Vec<_> = self.live.forward.iter().collect();
        live.sort_by(|a, b| a.0.cmp(b.0));
        live.into_iter()
    }
}

impl IndexStore for FileIndex {
    fn register(&mut self, id: &Id, path: &Path) {
        self.live.register(id, path);
        self.dirty = true;
    }

    fn resolve(&self, id: &Id) -> Option<PathBuf> {
        self.live.resolve(id)
    }

    fn id_for_path(&self, path: &Path) -> Option<Id> {
        self.live.id_for_path(path)
    }

    fn set_path(&mut self, id: &Id, new_path: &Path) {
        self.live.set_path(id, new_path);
        self.dirty = true;
    }

    /// Retire to a tombstone: the ID stops resolving but stays known forever.
    fn unregister(&mut self, id: &Id) {
        self.live.unregister(id);
        self.tombstones.insert(id.clone());
        self.dirty = true;
    }

    fn is_known(&self, id: &Id) -> bool {
        self.live.resolve(id).is_some() || self.tombstones.contains(id)
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

    #[test]
    fn file_index_round_trips_sorted_with_tombstones() {
        let mut ix = FileIndex::new(fig::Format::Yaml);
        ix.register(&Id("zzzzzzz".into()), Path::new("z.md"));
        ix.register(&Id("bcdfghj".into()), Path::new("notes/a.md"));
        ix.register(&Id("mmmmmmm".into()), Path::new("gone.md"));
        ix.unregister(&Id("mmmmmmm".into()));

        let text = ix.render().unwrap();
        // Sorted, one record per line, tombstone as null.
        let b = text.find("bcdfghj").unwrap();
        let m = text.find("mmmmmmm").unwrap();
        let z = text.find("zzzzzzz").unwrap();
        assert!(b < m && m < z, "{text}");
        assert!(text.contains("mmmmmmm: null"), "{text}");

        let back = FileIndex::parse(Path::new("registry.yaml"), &text).unwrap();
        assert_eq!(back.resolve(&Id("bcdfghj".into())), Some(PathBuf::from("notes/a.md")));
        assert_eq!(back.resolve(&Id("mmmmmmm".into())), None);
        assert!(back.is_known(&Id("mmmmmmm".into())), "tombstone survives the round-trip");
        assert!(back.is_tombstoned(&Id("mmmmmmm".into())));
        assert!(!back.is_dirty());
    }

    #[test]
    fn registry_host_keeps_its_self_description_and_comments() {
        // A bare config host with a title, a part_of back to the root, and a
        // comment: splicing records must leave all of that alone.
        let host = "# who am I? see title
title: ID registry
part_of: index.md
registry:
  bcdfghj: a.md
";
        let mut ix = FileIndex::parse(Path::new("registry.yaml"), host).unwrap();
        ix.register(&Id("zzzzzzz".into()), Path::new("z.md"));
        let out = ix.render().unwrap();
        assert!(out.contains("# who am I? see title"), "{out}");
        assert!(out.contains("title: ID registry"), "{out}");
        assert!(out.contains("part_of: index.md"), "{out}");
        assert!(out.contains("bcdfghj: a.md"), "{out}");
        assert!(out.contains("zzzzzzz: z.md"), "{out}");
    }

    #[test]
    fn registry_can_live_in_markdown_frontmatter() {
        // The registry embedded in a prose document: records in the fenced
        // block, body untouched.
        let host = "---
title: Registry
part_of: index.md
registry:
  bcdfghj: a.md
---
# About this file

The workspace's ID registry lives in my frontmatter.
";
        let mut ix = FileIndex::parse(Path::new("registry.md"), host).unwrap();
        assert_eq!(ix.resolve(&Id("bcdfghj".into())), Some(PathBuf::from("a.md")));
        ix.set_path(&Id("bcdfghj".into()), Path::new("moved/a.md"));
        let out = ix.render().unwrap();
        assert!(out.starts_with("---
title: Registry"), "fences kept: {out}");
        assert!(out.contains("bcdfghj: moved/a.md"), "{out}");
        assert!(out.ends_with("The workspace's ID registry lives in my frontmatter.\n"), "body kept: {out}");

        let back = FileIndex::parse(Path::new("registry.md"), &out).unwrap();
        assert_eq!(back.resolve(&Id("bcdfghj".into())), Some(PathBuf::from("moved/a.md")));
    }

    #[test]
    fn tombstoned_ids_are_never_free_for_reminting() {
        let mut ix = FileIndex::new(fig::Format::Yaml);
        let id = Id("bcdfghj".into());
        ix.register(&id, Path::new("a.md"));
        ix.unregister(&id);
        assert_eq!(ix.resolve(&id), None, "does not resolve");
        assert!(ix.is_known(&id), "but is still known — never reminted");
    }

    #[test]
    fn dirty_tracks_mutations() {
        let mut ix = FileIndex::new(fig::Format::Yaml);
        assert!(!ix.is_dirty());
        ix.register(&Id("x".into()), Path::new("a.md"));
        assert!(ix.is_dirty());
        ix.mark_clean();
        assert!(!ix.is_dirty());
    }

    #[test]
    fn empty_text_is_an_empty_registry() {
        let ix = FileIndex::parse(Path::new("registry.yaml"), "").unwrap();
        assert!(ix.is_empty());
    }
}
