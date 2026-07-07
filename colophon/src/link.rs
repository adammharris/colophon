//! Link text — the raw strings a relation field holds, the wikilinks embedded
//! in body prose, and the path arithmetic around them.
//!
//! A link target as written in metadata is either a bare relative path
//! (`notes/a.md`) or a markdown-style labeled link (`[Design](docs/design.md)`);
//! a [`Wikilink`] is the body-text counterpart (`[[notes/a.md]]`,
//! `[[colophon:ajp7eq|My file]]`). Everything here is *lexical*: no filesystem
//! access, no symlink resolution, and no markdown-structure awareness (a `[[…]]`
//! inside a code span is still scanned) — resolution and code-fence discipline
//! belong to the traversal and validation layers, which can report what they
//! find.

use std::ops::Range;
use std::path::{Component, Path, PathBuf};

/// A parsed link string: an optional human label and the target it points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// The display label, when written as `[label](target)`.
    pub label: Option<String>,
    /// The target exactly as written (a relative path, or a URL for overlay
    /// relations that point off-workspace).
    pub target: String,
}

impl Link {
    /// Parse a raw link string. `[label](target)` yields both parts; anything
    /// else is a bare target with no label.
    pub fn parse(raw: &str) -> Self {
        if let Some(rest) = raw.strip_prefix('[')
            && let Some(inner) = rest.strip_suffix(')')
            && let Some((label, target)) = inner.split_once("](")
        {
            return Self {
                label: Some(label.to_string()),
                target: target.to_string(),
            };
        }
        Self {
            label: None,
            target: raw.to_string(),
        }
    }

    /// Render back to the string form it was parsed from — labeled links keep
    /// their label, bare targets stay bare.
    pub fn render(&self) -> String {
        match &self.label {
            Some(label) => format!("[{label}]({})", self.target),
            None => self.target.clone(),
        }
    }

    /// This link with a different target, keeping the label. The rename path
    /// uses this so `[Design](old.md)` becomes `[Design](new.md)`, never a
    /// bare `new.md`.
    pub fn with_target(&self, target: impl Into<String>) -> Self {
        Self {
            label: self.label.clone(),
            target: target.into(),
        }
    }

    /// `true` when the target points off-workspace (a URL or mail address)
    /// rather than at a file — such links are never resolved against the
    /// filesystem or rewritten by moves.
    pub fn is_external(&self) -> bool {
        self.target.contains("://") || self.target.starts_with("mailto:")
    }

    /// The stable ID this link names, when the target uses the
    /// `colophon:<id>` scheme — the location-independent alternative to a
    /// relative path. Such targets resolve through the workspace's ID
    /// registry, never against the filesystem, and are deliberately *not*
    /// rewritten by moves: staying valid across moves is their entire point.
    pub fn id_target(&self) -> Option<crate::identity::Id> {
        self.target
            .strip_prefix(ID_SCHEME)
            .map(|id| crate::identity::Id(id.to_string()))
    }
}

/// The target scheme marking a link-by-ID: `colophon:<id>`.
pub const ID_SCHEME: &str = "colophon:";

/// Render an ID as a link target (`colophon:<id>`).
pub fn id_target(id: &crate::identity::Id) -> String {
    format!("{ID_SCHEME}{id}")
}

/// A wikilink embedded in a document's body: `[[target]]` or, with an Obsidian
/// pipe label, `[[target|label]]`.
///
/// This is the body-text sibling of a metadata [`Link`]. The `target` is either
/// a path (`[[notes/a.md]]`, the identity-free Diaryx-style link that moves
/// rewrite) or a `colophon:<id>` reference (`[[colophon:ajp7eq]]`, the
/// location-independent Obsidian-style link that moves leave alone — the
/// registry update is its maintenance). Which one a workspace mints is a policy
/// choice; discovering the span is not, so the scanner is neutral between them.
///
/// [`span`](Wikilink::span) is the byte range of the whole `[[…]]` construct in
/// the source body — exactly what a rewrite replaces, so a path retarget can
/// splice a new target back in without re-parsing the prose around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wikilink {
    /// The target as written between `[[` and the `|` (or the closing `]]`),
    /// trimmed of surrounding whitespace.
    pub target: String,
    /// The display label after `|`, when written `[[target|label]]`.
    pub label: Option<String>,
    /// Byte range of the entire `[[…]]` span within the scanned body.
    pub span: Range<usize>,
}

impl Wikilink {
    /// Build a wikilink from the raw inner text (between `[[` and `]]`) and its
    /// span. Splits an Obsidian `target|label` on the first `|`; an empty target
    /// (e.g. `[[]]` or `[[ | x ]]`) is not a link and yields `None`.
    fn from_inner(inner: &str, span: Range<usize>) -> Option<Self> {
        let (target, label) = match inner.split_once('|') {
            Some((target, label)) => (target.trim(), Some(label.trim().to_string())),
            None => (inner.trim(), None),
        };
        if target.is_empty() {
            return None;
        }
        Some(Self {
            target: target.to_string(),
            label,
            span,
        })
    }

    /// Render back to `[[target]]` / `[[target|label]]`. Surrounding whitespace
    /// inside the brackets is not preserved — the rendered form is canonical.
    pub fn render(&self) -> String {
        match &self.label {
            Some(label) => format!("[[{}|{label}]]", self.target),
            None => format!("[[{}]]", self.target),
        }
    }

    /// This wikilink with a different target, keeping the label — the move path
    /// uses it to rewrite a *path* target while leaving the display text intact.
    /// (ID targets are never rewritten; that is the whole point of using one.)
    pub fn with_target(&self, target: impl Into<String>) -> Self {
        Self {
            target: target.into(),
            label: self.label.clone(),
            span: self.span.clone(),
        }
    }

    /// The stable ID this wikilink names, when its target uses the
    /// `colophon:<id>` scheme — `None` for a plain path target. Mirrors
    /// [`Link::id_target`].
    pub fn id_target(&self) -> Option<crate::identity::Id> {
        self.target
            .strip_prefix(ID_SCHEME)
            .map(|id| crate::identity::Id(id.to_string()))
    }
}

/// Scan body prose for every `[[…]]` wikilink, in source order, each carrying
/// its byte span. Purely lexical: unclosed `[[` is ignored, the first following
/// `]]` closes the span, and no markdown structure (code spans, escapes) is
/// interpreted — a higher layer decides whether a match in a code fence counts.
pub fn parse_wikilinks(body: &str) -> Vec<Wikilink> {
    let mut out = Vec::new();
    let mut base = 0; // byte offset of `rest` within `body`
    let mut rest = body;
    while let Some(open_rel) = rest.find("[[") {
        let open = base + open_rel;
        let after_open = open + 2;
        let Some(close_rel) = body[after_open..].find("]]") else {
            break; // no closing delimiter anywhere ahead — nothing more to find
        };
        let close = after_open + close_rel;
        if let Some(link) = Wikilink::from_inner(&body[after_open..close], open..close + 2) {
            out.push(link);
        }
        base = close + 2;
        rest = &body[base..];
    }
    out
}

/// Lexically normalize a relative path: drop `.` components and fold
/// `parent/..` pairs. Leading `..` components (escaping the workspace root)
/// are kept — the caller decides whether that is an error.
pub fn normalize(path: impl AsRef<Path>) -> PathBuf {
    let mut out: Vec<Component> = Vec::new();
    for component in path.as_ref().components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                _ => out.push(component),
            },
            other => out.push(other),
        }
    }
    out.iter().collect()
}

/// Resolve a link target written in `doc` against `doc`'s directory, yielding
/// a normalized path in the same coordinate system as `doc` (workspace-relative
/// when `doc` is workspace-relative).
pub fn resolve(doc: &Path, target: &str) -> PathBuf {
    let dir = doc.parent().unwrap_or(Path::new(""));
    normalize(dir.join(target))
}

/// The relative path string that reaches `to` from `from_dir` (both normalized,
/// same coordinate system). Rendered with forward slashes — link targets are
/// text, not platform paths.
pub fn relative(from_dir: &Path, to: &Path) -> String {
    let from: Vec<&std::ffi::OsStr> = from_dir.iter().collect();
    let to_parts: Vec<&std::ffi::OsStr> = to.iter().collect();
    let common = from
        .iter()
        .zip(to_parts.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut parts: Vec<String> = Vec::new();
    for _ in common..from.len() {
        parts.push("..".to_string());
    }
    for part in &to_parts[common..] {
        parts.push(part.to_string_lossy().into_owned());
    }
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_labeled_and_bare_links() {
        let l = Link::parse("[Design](docs/design.md)");
        assert_eq!(l.label.as_deref(), Some("Design"));
        assert_eq!(l.target, "docs/design.md");
        assert_eq!(l.render(), "[Design](docs/design.md)");

        let bare = Link::parse("notes/a.md");
        assert_eq!(bare.label, None);
        assert_eq!(bare.render(), "notes/a.md");
    }

    #[test]
    fn odd_shapes_fall_back_to_bare() {
        // A target with brackets but not the [label](target) shape.
        for raw in ["[unclosed](x", "no[mid](x)", "[]"] {
            assert_eq!(Link::parse(raw).render(), raw);
        }
    }

    #[test]
    fn with_target_keeps_the_label() {
        let l = Link::parse("[Design](old.md)").with_target("new.md");
        assert_eq!(l.render(), "[Design](new.md)");
    }

    #[test]
    fn external_links_are_flagged() {
        assert!(Link::parse("https://example.com/x").is_external());
        assert!(Link::parse("[me](mailto:a@b.c)").is_external());
        assert!(!Link::parse("docs/design.md").is_external());
    }

    #[test]
    fn normalizes_dot_and_dotdot() {
        assert_eq!(normalize("a/./b/../c.md"), PathBuf::from("a/c.md"));
        assert_eq!(normalize("../up.md"), PathBuf::from("../up.md"));
        assert_eq!(normalize("a/b/../../x.md"), PathBuf::from("x.md"));
    }

    #[test]
    fn resolves_against_the_documents_directory() {
        assert_eq!(
            resolve(Path::new("docs/index.md"), "../README.md"),
            PathBuf::from("README.md")
        );
        assert_eq!(
            resolve(Path::new("README.md"), "docs/design.md"),
            PathBuf::from("docs/design.md")
        );
    }

    #[test]
    fn scans_bare_and_labeled_wikilinks_with_spans() {
        let body = "see [[notes/a.md]] and [[colophon:ajp7eq|My file]] here";
        let links = parse_wikilinks(body);
        assert_eq!(links.len(), 2);

        assert_eq!(links[0].target, "notes/a.md");
        assert_eq!(links[0].label, None);
        assert_eq!(&body[links[0].span.clone()], "[[notes/a.md]]");
        assert_eq!(links[0].id_target(), None);

        assert_eq!(links[1].target, "colophon:ajp7eq");
        assert_eq!(links[1].label.as_deref(), Some("My file"));
        assert_eq!(&body[links[1].span.clone()], "[[colophon:ajp7eq|My file]]");
        assert_eq!(links[1].id_target(), Some(crate::identity::Id("ajp7eq".into())));
    }

    #[test]
    fn wikilink_scan_trims_and_skips_degenerate_shapes() {
        // Whitespace inside the brackets is trimmed on both sides of the pipe.
        let trimmed = parse_wikilinks("x [[  notes/a.md  |  Label  ]] y");
        assert_eq!(trimmed[0].target, "notes/a.md");
        assert_eq!(trimmed[0].label.as_deref(), Some("Label"));

        // Empty target and unclosed openers are not links.
        assert!(parse_wikilinks("nothing [[]] here").is_empty());
        assert!(parse_wikilinks("[[ | orphan label ]]").is_empty());
        assert!(parse_wikilinks("dangling [[notes/a.md without close").is_empty());
    }

    #[test]
    fn wikilink_render_round_trips_and_retargets() {
        let link = &parse_wikilinks("[[old.md|Design]]")[0];
        assert_eq!(link.render(), "[[old.md|Design]]");
        // Retarget keeps the label — the rename path relies on this.
        assert_eq!(link.with_target("new.md").render(), "[[new.md|Design]]");

        let bare = &parse_wikilinks("[[old.md]]")[0];
        assert_eq!(bare.render(), "[[old.md]]");
    }

    #[test]
    fn relative_walks_up_and_down() {
        assert_eq!(relative(Path::new("docs"), Path::new("README.md")), "../README.md");
        assert_eq!(relative(Path::new(""), Path::new("docs/design.md")), "docs/design.md");
        assert_eq!(relative(Path::new("a/b"), Path::new("a/b/c.md")), "c.md");
        assert_eq!(relative(Path::new("a/b"), Path::new("a/b")), ".");
    }
}
