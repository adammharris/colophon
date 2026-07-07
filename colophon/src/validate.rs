//! Validation — integrity findings over the workspace graph, from a root.
//!
//! The sleeper feature (DESIGN §8): walk the spanning tree and report every
//! violated invariant as a [`Finding`] — data, not a panic.
//!
//! Underneath sits the **census** ([`Workspace::census`]): one traversal that
//! yields every forward link reachable from the root — frontmatter relation
//! edges *and* body `[[…]]` wikilinks alike — each tagged with where it is
//! written ([`LinkSite`]) and how it resolves ([`Resolution`]). Because it is
//! read straight from the documents, the census is *ground truth*; the backlink
//! map, these findings, and (in `mutate`) inbound-rename maintenance are all
//! views over it, and any stored index heals toward it. [`Workspace::check`] is
//! the findings view. The checks:
//!
//! - **broken link** — a path target (in a relation or a wikilink) that
//!   resolves to nothing on disk;
//! - **case mismatch** — a target that only resolves because the filesystem is
//!   case-insensitive (`docs/design.md` vs `docs/DESIGN.md`): works on macOS,
//!   breaks on Linux. Caught by comparing exact directory listings;
//! - **cycle / duplicate containment** — a spanning target already visited
//!   (the spanning relation must be a single-parent tree);
//! - **missing inverse** — a spanning child whose inverse field (`part_of`)
//!   does not point back at its parent;
//! - **malformed / dangling ID** — a `colophon:<id>` reference (in a relation
//!   or a wikilink) that fails its check character, or that no live registry
//!   entry resolves;
//! - **unreadable** — a document that exists but cannot be read or parsed.
//!
//! External targets (URLs, `mailto:`) are never checked. Autofix comes with
//! the mutation layer's growth; findings first.

use std::collections::BTreeSet;
use std::fmt;
use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::fs::Storage;
use crate::identity::{self, Id};
use crate::index::IndexStore;
use crate::link::{self, Link};
use crate::meta::Value;
use crate::workspace::{Target, Workspace};

/// Where in a document a forward link is written — a frontmatter relation field
/// or a body wikilink. Carried by every link-resolution [`Finding`] and every
/// [`CensusEntry`] so a report can point at the exact site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkSite {
    /// A frontmatter relation field, by name (e.g. `contents`, `links`).
    Relation(String),
    /// A `[[…]]` wikilink in the body, at this byte span.
    Body(Range<usize>),
}

impl fmt::Display for LinkSite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LinkSite::Relation(name) => f.write_str(name),
            LinkSite::Body(_) => f.write_str("body"),
        }
    }
}

/// How a forward link resolves against the workspace. Path and id forms stay
/// distinct on purpose: the registry owns id resolution (location-independent,
/// stable across moves), while a path is checked against the on-disk name — so
/// a caller can tell which links a rename must rewrite (paths) from which it
/// must leave alone (ids).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// A path target that resolves to an existing file (exact name).
    Path(PathBuf),
    /// A path target that only matches case-insensitively; `got` is the target
    /// as resolved, `actual` the exact on-disk name.
    CaseMismatch { got: PathBuf, actual: String },
    /// A path target with nothing on disk.
    Broken,
    /// A `colophon:<id>` target the registry resolves to the live path `to`.
    Id { id: Id, to: PathBuf },
    /// A well-formed `colophon:<id>` target with no live registry entry;
    /// `tombstoned` separates "deleted" from "never issued here" (§4 hazard).
    DanglingId { id: Id, tombstoned: bool },
    /// A `colophon:<id>` target failing its check character — a typo.
    MalformedId,
    /// A URL / mail address — off-workspace, never resolved or rewritten.
    External,
}

impl Resolution {
    /// The workspace path this link reaches, if it resolves to one (by path or
    /// through the registry) — what the spanning walk descends into and what a
    /// backlink map keys on. `None` for broken, dangling, malformed, external.
    pub fn resolved_path(&self) -> Option<&PathBuf> {
        match self {
            Resolution::Path(p)
            | Resolution::CaseMismatch { got: p, .. }
            | Resolution::Id { to: p, .. } => Some(p),
            _ => None,
        }
    }
}

/// One forward link as found in a document: where it is written and how it
/// resolves. The unit of the [`census`](Workspace::census).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CensusEntry {
    /// The document that declares the link (workspace-relative).
    pub source: PathBuf,
    /// Where in `source` the link is written.
    pub site: LinkSite,
    /// The target exactly as written.
    pub target_text: String,
    /// How the target resolves.
    pub resolution: Resolution,
}

impl CensusEntry {
    /// The integrity finding this entry represents when its target failed to
    /// resolve cleanly — `None` for a link that resolves.
    fn finding(&self) -> Option<Finding> {
        let doc = self.source.clone();
        let site = self.site.clone();
        let target = self.target_text.clone();
        match &self.resolution {
            Resolution::CaseMismatch { actual, .. } => {
                Some(Finding::CaseMismatch { doc, site, target, actual: actual.clone() })
            }
            Resolution::Broken => Some(Finding::BrokenLink { doc, site, target }),
            Resolution::MalformedId => Some(Finding::MalformedId { doc, site, target }),
            Resolution::DanglingId { id, tombstoned } => {
                Some(Finding::DanglingId { doc, site, id: id.clone(), tombstoned: *tombstoned })
            }
            Resolution::Path(_) | Resolution::Id { .. } | Resolution::External => None,
        }
    }
}

/// One integrity finding. `doc` is always the document that *declares* the
/// problem (workspace-relative); `site` is where in it the offending link sits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// `target` (written at `site`) resolves to nothing on disk.
    BrokenLink { doc: PathBuf, site: LinkSite, target: String },
    /// `target` only resolves case-insensitively; the exact on-disk name is
    /// `actual`. Portable workspaces need the exact name.
    CaseMismatch { doc: PathBuf, site: LinkSite, target: String, actual: String },
    /// A spanning target that was already reached — a containment cycle or a
    /// second parent, either of which breaks the single-parent spanning tree.
    DuplicateContainment { doc: PathBuf, target: String },
    /// A spanning child whose inverse field does not link back to `doc`.
    MissingInverse { doc: PathBuf, child: PathBuf, inverse: String },
    /// A document that exists but could not be read or parsed.
    Unreadable { doc: PathBuf, error: String },
    /// A `colophon:<id>` reference whose ID fails the shape/check-character
    /// test — almost certainly a typo, caught before it dangles silently.
    MalformedId { doc: PathBuf, site: LinkSite, target: String },
    /// A well-formed `colophon:<id>` reference with no live registry entry.
    /// `tombstoned` distinguishes "that document was deleted" from "this ID
    /// was never issued here" (an out-of-band reference the registry has not
    /// reconciled — DESIGN §4's known hazard).
    DanglingId { doc: PathBuf, site: LinkSite, id: Id, tombstoned: bool },
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Finding::BrokenLink { doc, site, target } => {
                write!(f, "{}: broken {site} link: {target}", doc.display())
            }
            Finding::CaseMismatch { doc, site, target, actual } => write!(
                f,
                "{}: case mismatch in {site} link: {target} is {actual} on disk",
                doc.display()
            ),
            Finding::DuplicateContainment { doc, target } => write!(
                f,
                "{}: {target} is already contained elsewhere (cycle or second parent)",
                doc.display()
            ),
            Finding::MissingInverse { doc, child, inverse } => write!(
                f,
                "{}: child {} does not declare {inverse} back to it",
                doc.display(),
                child.display()
            ),
            Finding::Unreadable { doc, error } => {
                write!(f, "{}: unreadable: {error}", doc.display())
            }
            Finding::MalformedId { doc, site, target } => write!(
                f,
                "{}: malformed ID in {site} link: {target} (bad shape or check character)",
                doc.display()
            ),
            Finding::DanglingId { doc, site, id, tombstoned } => write!(
                f,
                "{}: dangling {site} ID: colophon:{id} ({})",
                doc.display(),
                if *tombstoned { "document was deleted" } else { "never issued in this registry" }
            ),
        }
    }
}

impl<FS: Storage, IdP, Ix: IndexStore> Workspace<FS, IdP, Ix> {
    /// Check the workspace reachable from `start`, returning every finding.
    /// An empty result means the reachable graph holds its invariants. This is
    /// the findings view over [`census`](Workspace::census): each forward link
    /// that fails to resolve becomes a finding, joined with the structural
    /// findings (unreadable document, duplicate containment, missing inverse)
    /// the walk raises from traversal state.
    pub async fn check(&self, start: impl AsRef<Path>) -> Result<Vec<Finding>> {
        let (census, mut findings) = self.walk(start.as_ref()).await?;
        for entry in &census {
            findings.extend(entry.finding());
        }
        Ok(findings)
    }

    /// Take a census of every forward link reachable from `start`: one
    /// [`CensusEntry`] per frontmatter relation edge *and* per body `[[…]]`
    /// wikilink, each carrying its [`LinkSite`] and [`Resolution`].
    ///
    /// This is the one traversal the backlink map, the integrity findings, and
    /// (via `mutate`) inbound-rename maintenance are all views over. Because it
    /// is read from the documents, it is ground truth: a stored backlink index
    /// heals *toward* the census, never the reverse.
    pub async fn census(&self, start: impl AsRef<Path>) -> Result<Vec<CensusEntry>> {
        Ok(self.walk(start.as_ref()).await?.0)
    }

    /// The shared spanning-tree walk: gathers the forward-link census and the
    /// structural findings (which depend on traversal state, not on a single
    /// link's resolution) in one pass. Frontmatter edges may be spanning and so
    /// drive descent, the single-parent check, and the inverse check; body
    /// wikilinks are always overlay references — censused, never spanning.
    async fn walk(&self, start: &Path) -> Result<(Vec<CensusEntry>, Vec<Finding>)> {
        let mut census = Vec::new();
        let mut structural = Vec::new();
        let mut visited = BTreeSet::new();
        let mut queue = vec![link::normalize(start)];

        let spanning = self.relations().spanning_relation().map(str::to_owned);
        let inverse = spanning.as_deref().and_then(|s| {
            self.relations()
                .relations()
                .iter()
                .find(|r| r.name == s)
                .and_then(|r| r.inverse.clone())
        });

        while let Some(path) = queue.pop() {
            if !visited.insert(path.clone()) {
                continue;
            }
            let doc = match self.load(&path).await {
                Ok((_, doc)) => doc,
                Err(e) => {
                    structural.push(Finding::Unreadable { doc: path, error: e.to_string() });
                    continue;
                }
            };

            // Frontmatter relation edges — the only links that can be spanning.
            for edge in self.relations().edges(&doc.meta) {
                // Parse once: `link.target` is the bare target (any `[label](…)`
                // stripped), which is what both the census and findings record.
                let link = Link::parse(&edge.target);
                let resolution = self.resolve_forward(&path, &link).await;

                if Some(edge.relation.as_str()) == spanning.as_deref()
                    && let Some(resolved) = resolution.resolved_path().cloned()
                {
                    // Single-parent check, inverse check, descent.
                    if visited.contains(&resolved) || queue.contains(&resolved) {
                        structural.push(Finding::DuplicateContainment {
                            doc: path.clone(),
                            target: link.target.clone(),
                        });
                    } else {
                        if let Some(inverse) = inverse.as_deref()
                            && let Ok((_, child_doc)) = self.load(&resolved).await
                            && child_doc.has_meta()
                        {
                            let points_back = child_doc
                                .meta
                                .get(inverse)
                                .map(Value::link_strings)
                                .unwrap_or_default()
                                .iter()
                                .any(|t| {
                                    self.resolve_link(&resolved, &Link::parse(t))
                                        == Target::Path(path.clone())
                                });
                            if !points_back {
                                structural.push(Finding::MissingInverse {
                                    doc: path.clone(),
                                    child: resolved.clone(),
                                    inverse: inverse.to_string(),
                                });
                            }
                        }
                        queue.push(resolved);
                    }
                }

                census.push(CensusEntry {
                    source: path.clone(),
                    site: LinkSite::Relation(edge.relation),
                    target_text: link.target,
                    resolution,
                });
            }

            // Body wikilinks — overlay references, censused but never spanning.
            for wikilink in link::parse_wikilinks(&doc.body) {
                let resolution = self.resolve_forward(&path, &Link::parse(&wikilink.target)).await;
                census.push(CensusEntry {
                    source: path.clone(),
                    site: LinkSite::Body(wikilink.span),
                    target_text: wikilink.target,
                    resolution,
                });
            }
        }
        Ok((census, structural))
    }

    /// Resolve one forward link (declared in the document at `source`) into a
    /// [`Resolution`]. A path target is checked against the on-disk name; a
    /// `colophon:<id>` target resolves through the registry and stays an
    /// id-form resolution, so callers can distinguish a location-independent
    /// link (never rewritten by a move) from a path (which is).
    async fn resolve_forward(&self, source: &Path, link: &Link) -> Resolution {
        if link.is_external() {
            return Resolution::External;
        }
        if let Some(id) = link.id_target() {
            if !identity::verify(id.as_str()) {
                return Resolution::MalformedId;
            }
            return match self.index().resolve(&id) {
                Some(path) => Resolution::Id { id, to: link::normalize(path) },
                None => Resolution::DanglingId { tombstoned: self.index().is_known(&id), id },
            };
        }
        let resolved = link::resolve(source, &link.target);
        match self.exact_name(&resolved).await {
            NameMatch::Exact => Resolution::Path(resolved),
            NameMatch::CaseOnly(actual) => Resolution::CaseMismatch { got: resolved, actual },
            NameMatch::None => Resolution::Broken,
        }
    }

    /// How `path`'s final component matches its parent directory's listing:
    /// exactly, only case-insensitively (the portability hazard), or not at all.
    async fn exact_name(&self, path: &Path) -> NameMatch {
        let full = self.root().join(path);
        let (Some(parent), Some(name)) = (full.parent(), full.file_name()) else {
            return NameMatch::None;
        };
        let Ok(entries) = self.fs().read_dir(parent).await else {
            return NameMatch::None;
        };
        let mut case_only = None;
        for entry in entries {
            let Some(entry_name) = entry.file_name() else { continue };
            if entry_name == name {
                return NameMatch::Exact;
            }
            if entry_name.eq_ignore_ascii_case(name) {
                case_only = Some(entry_name.to_string_lossy().into_owned());
            }
        }
        match case_only {
            Some(actual) => NameMatch::CaseOnly(actual),
            None => NameMatch::None,
        }
    }
}

enum NameMatch {
    Exact,
    CaseOnly(String),
    None,
}

// These tests use YAML frontmatter fixtures, so they run under the `yaml` feature.
#[cfg(all(test, feature = "yaml"))]
mod tests {
    use super::*;
    use crate::exec::block_on;
    use crate::fs::StdFs;

    fn write(dir: &Path, rel: &str, text: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn tempdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("colophon-check-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_clean_workspace_has_no_findings() {
        let dir = tempdir("clean");
        write(&dir, "index.md", "---\ncontents:\n- a.md\n---\n");
        write(&dir, "a.md", "---\npart_of: index.md\n---\n");
        let ws = Workspace::builder(StdFs).root(&dir).build();
        assert_eq!(block_on(ws.check("index.md")).unwrap(), vec![]);
    }

    #[test]
    fn broken_case_mismatched_and_uninversed_links_are_found() {
        let dir = tempdir("dirty");
        write(
            &dir,
            "index.md",
            "---\ncontents:\n- gone.md\n- '[D](docs/design.md)'\n- b.md\n---\n",
        );
        write(&dir, "docs/DESIGN.md", "---\npart_of: ../index.md\n---\n");
        write(&dir, "b.md", "---\ntitle: no part_of here\n---\n");

        let ws = Workspace::builder(StdFs).root(&dir).build();
        let findings = block_on(ws.check("index.md")).unwrap();
        assert!(
            findings.iter().any(|f| matches!(f, Finding::BrokenLink { target, .. } if target == "gone.md")),
            "{findings:?}"
        );
        assert!(
            findings.iter().any(|f| matches!(
                f,
                Finding::CaseMismatch { target, actual, .. } if target == "docs/design.md" && actual == "DESIGN.md"
            )),
            "{findings:?}"
        );
        assert!(
            findings.iter().any(|f| matches!(
                f,
                Finding::MissingInverse { child, .. } if child == &PathBuf::from("b.md")
            )),
            "{findings:?}"
        );
    }

    #[test]
    fn census_covers_frontmatter_edges_and_body_wikilinks() {
        let dir = tempdir("census");
        write(
            &dir,
            "index.md",
            "---\ncontents:\n- a.md\n---\nBody links [[a.md]] and [[gone.md]].\n",
        );
        write(&dir, "a.md", "---\npart_of: index.md\n---\n");
        let ws = Workspace::builder(StdFs).root(&dir).build();
        let census = block_on(ws.census("index.md")).unwrap();

        // The frontmatter `contents` edge, resolving to the existing file.
        assert!(
            census.iter().any(|e| matches!(&e.site, LinkSite::Relation(r) if r == "contents")
                && matches!(&e.resolution, Resolution::Path(p) if p == &PathBuf::from("a.md"))),
            "{census:?}"
        );
        // The body wikilink to the same file — sited in the body, resolving.
        assert!(
            census.iter().any(|e| matches!(e.site, LinkSite::Body(_))
                && e.target_text == "a.md"
                && matches!(&e.resolution, Resolution::Path(_))),
            "{census:?}"
        );
        // The body wikilink to a missing file — a Broken resolution.
        assert!(
            census.iter().any(|e| e.target_text == "gone.md"
                && matches!(e.resolution, Resolution::Broken)),
            "{census:?}"
        );
    }

    #[test]
    fn check_flags_a_broken_body_wikilink() {
        let dir = tempdir("body-broken");
        write(&dir, "index.md", "---\ntitle: Root\n---\nSee [[gone.md]] for more.\n");
        let ws = Workspace::builder(StdFs).root(&dir).build();
        let findings = block_on(ws.check("index.md")).unwrap();
        assert!(
            findings.iter().any(|f| matches!(
                f,
                Finding::BrokenLink { site: LinkSite::Body(_), target, .. } if target == "gone.md"
            )),
            "{findings:?}"
        );
    }

    #[test]
    fn a_resolving_body_wikilink_is_not_a_finding() {
        let dir = tempdir("body-clean");
        write(&dir, "index.md", "---\ncontents:\n- a.md\n---\nSee [[a.md]].\n");
        write(&dir, "a.md", "---\npart_of: index.md\n---\n");
        let ws = Workspace::builder(StdFs).root(&dir).build();
        assert_eq!(block_on(ws.check("index.md")).unwrap(), vec![]);
    }

    #[test]
    fn duplicate_containment_is_found() {
        let dir = tempdir("dup");
        write(&dir, "index.md", "---\ncontents:\n- a.md\n- b.md\n---\n");
        write(&dir, "a.md", "---\npart_of: index.md\ncontents:\n- b.md\n---\n");
        write(&dir, "b.md", "---\npart_of: index.md\n---\n");
        let ws = Workspace::builder(StdFs).root(&dir).build();
        let findings = block_on(ws.check("index.md")).unwrap();
        assert!(
            findings.iter().any(|f| matches!(f, Finding::DuplicateContainment { .. })),
            "{findings:?}"
        );
    }
}
