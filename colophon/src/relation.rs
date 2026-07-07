//! Relations — the configurable vocabulary of links declared in metadata.
//!
//! colophon is opinionated about the *mechanism* (links live in embedded
//! metadata; one relation is the canonical tree; the rest overlay it) but not
//! about the *vocabulary*. A [`RelationSet`] names which fields are links, their
//! cardinality, their inverse, and which single relation is **spanning**.

use crate::meta::Value;

/// How many targets a relation field may hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cardinality {
    /// At most one target (e.g. a single-parent `part_of`).
    One,
    /// Any number of targets (e.g. `contents`, `links`).
    Many,
}

/// A single named relation: the frontmatter key it reads, its inverse (if the
/// pair is maintained bidirectionally), and its cardinality.
#[derive(Debug, Clone)]
pub struct Relation {
    /// The frontmatter key this relation reads (e.g. `"contents"`).
    pub name: String,
    /// The inverse relation's name, if any (e.g. `contents` ↔ `part_of`).
    pub inverse: Option<String>,
    /// How many targets the field may hold.
    pub cardinality: Cardinality,
}

impl Relation {
    /// A single-valued relation (cardinality [`Cardinality::One`]).
    pub fn one(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            inverse: None,
            cardinality: Cardinality::One,
        }
    }

    /// A multi-valued relation (cardinality [`Cardinality::Many`]).
    pub fn many(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            inverse: None,
            cardinality: Cardinality::Many,
        }
    }

    /// Declare this relation's inverse (builder-style).
    pub fn inverse(mut self, name: impl Into<String>) -> Self {
        self.inverse = Some(name.into());
        self
    }
}

/// A resolved link found in a document's metadata: which relation declared it
/// and the raw (unresolved) target string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    /// The relation (frontmatter key) that declared this link.
    pub relation: String,
    /// The raw target string exactly as written in the metadata.
    pub target: String,
}

/// The configured set of relations for a workspace, and which one is spanning.
///
/// The **spanning** relation is the single-parent containment tree that gives
/// the workspace its self-describing discovery spine. All other relations may
/// be many-to-many overlays.
#[derive(Debug, Clone, Default)]
pub struct RelationSet {
    relations: Vec<Relation>,
    spanning: Option<String>,
}

impl RelationSet {
    /// An empty relation set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a relation (builder-style).
    pub fn with(mut self, relation: Relation) -> Self {
        self.relations.push(relation);
        self
    }

    /// Mark the named relation as the spanning (canonical tree) relation.
    pub fn spanning(mut self, name: impl Into<String>) -> Self {
        self.spanning = Some(name.into());
        self
    }

    /// The diaryx vocabulary: `contents`/`part_of` containment (spanning) plus
    /// `links`/`link_of` arbitrary cross-references.
    pub fn diaryx() -> Self {
        Self::new()
            .with(Relation::many("contents").inverse("part_of"))
            .with(Relation::one("part_of").inverse("contents"))
            .with(Relation::many("links").inverse("link_of"))
            .with(Relation::many("link_of").inverse("links"))
            .spanning("contents")
    }

    /// The configured relations.
    pub fn relations(&self) -> &[Relation] {
        &self.relations
    }

    /// The name of the spanning relation, if one is configured.
    pub fn spanning_relation(&self) -> Option<&str> {
        self.spanning.as_deref()
    }

    /// Extract every link declared by a document's metadata, tagged by relation.
    pub fn edges(&self, meta: &Value) -> Vec<Edge> {
        let mut edges = Vec::new();
        for relation in &self.relations {
            let Some(value) = meta.get(&relation.name) else {
                continue;
            };
            for target in value.link_strings() {
                edges.push(Edge {
                    relation: relation.name.clone(),
                    target,
                });
            }
        }
        edges
    }

    /// The raw targets of the spanning relation — i.e. this node's children in
    /// the canonical tree. Empty if no spanning relation is configured or the
    /// field is absent.
    pub fn children(&self, meta: &Value) -> Vec<String> {
        match self.spanning.as_deref().and_then(|name| meta.get(name)) {
            Some(value) => value.link_strings(),
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;

    fn doc(text: &str) -> Document {
        Document::parse("index.md", text).unwrap()
    }

    #[test]
    fn extracts_edges_tagged_by_relation() {
        let d = doc("---\ncontents:\n- a.md\n- b.md\npart_of: ../root.md\n---\nbody\n");
        let set = RelationSet::diaryx();
        let edges = set.edges(&d.meta);
        assert_eq!(edges.len(), 3);
        assert!(edges.contains(&Edge { relation: "contents".into(), target: "a.md".into() }));
        assert!(edges.contains(&Edge { relation: "part_of".into(), target: "../root.md".into() }));
    }

    #[test]
    fn children_reads_the_spanning_relation() {
        let d = doc("---\ncontents:\n- a.md\n- b.md\n---\nbody\n");
        let set = RelationSet::diaryx();
        assert_eq!(set.children(&d.meta), vec!["a.md".to_string(), "b.md".to_string()]);
        assert_eq!(set.spanning_relation(), Some("contents"));
    }

    #[test]
    fn custom_vocabulary_is_honored() {
        // Nothing diaryx-specific: organize by `part` / `whole`.
        let set = RelationSet::new()
            .with(Relation::many("part").inverse("whole"))
            .with(Relation::one("whole").inverse("part"))
            .spanning("part");
        let d = doc("---\npart:\n- one.md\n- two.md\n---\nbody\n");
        assert_eq!(set.children(&d.meta), vec!["one.md".to_string(), "two.md".to_string()]);
    }
}
