//! Documents — a plaintext file with an embedded metadata block and a body.

use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::meta::{self, Value};

/// The embed archetype a document's metadata block was found in — `---` YAML
/// frontmatter, `;;;` JSON frontmatter, a ```` ```fig ```` fenced block, or a
/// trailing ```` ```endmatter ```` block. Re-exported from `fig`, which owns
/// both detection (`fig::detect`) and the fence/format coupling
/// ([`EmbedType::inner_format`]).
pub use fig::EmbedType;

/// A parsed document: its path, its embedded metadata, and its body text.
///
/// Metadata is stored as a dynamic [`Value`] (a mapping, or [`Value::Null`] when
/// the document has no frontmatter) because link fields are configurable and
/// therefore accessed dynamically.
#[derive(Debug, Clone)]
pub struct Document {
    /// Path this document was read from (workspace-relative or absolute — the
    /// caller decides; colophon does not interpret it here).
    pub path: PathBuf,
    /// Parsed embedded metadata.
    pub meta: Value,
    /// Everything outside the metadata block (the host prose).
    pub body: String,
    /// The embed archetype the metadata was found in, or `None` when the
    /// document has no (well-formed) metadata block. Recorded at parse time so
    /// a write can preserve the original fence style and inner format — a
    /// ```` ```fig ```` block is never rewritten as `---` YAML.
    pub embed: Option<EmbedType>,
}

impl Document {
    /// Parse a document from its full text. The embedded metadata block is
    /// auto-detected via `fig::detect` — any archetype fig knows (`---` YAML,
    /// `;;;` JSON, ```` ```fig ````, ```` ```endmatter ````) — and parsed in
    /// that archetype's inner format. If there is no (well-formed) block,
    /// `meta` is [`Value::Null`] and the whole text is the body. An
    /// unterminated opening fence is treated as no metadata — we do not guess
    /// where it ends.
    pub fn parse(path: impl Into<PathBuf>, text: &str) -> Result<Self> {
        let (meta, body, embed) = match fig::detect(text) {
            Some(kind) => match fig::split(text, kind) {
                Some((content, body)) => (
                    meta::parse_value(content, kind.inner_format())?,
                    body.to_owned(),
                    Some(kind),
                ),
                // Detected by its open delimiter but with no matching close:
                // recognized-but-malformed degrades to "no metadata".
                None => (Value::Null, text.to_owned(), None),
            },
            None => (Value::Null, text.to_owned(), None),
        };
        Ok(Self {
            path: path.into(),
            meta,
            body,
            embed,
        })
    }

    /// The document's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `true` if the document declares any embedded metadata mapping.
    pub fn has_meta(&self) -> bool {
        self.meta.as_mapping().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_yaml_frontmatter_and_body() {
        let text = "---\ntitle: Root\ncontents:\n- a.md\n---\n# Body\n\nhello\n";
        let doc = Document::parse("index.md", text).unwrap();
        assert_eq!(doc.meta.get("title").and_then(Value::as_str), Some("Root"));
        assert_eq!(doc.body, "# Body\n\nhello\n");
        assert_eq!(doc.embed, Some(EmbedType::FrontmatterYaml));
        assert!(doc.has_meta());
    }

    #[test]
    fn parses_fig_fenced_frontmatter() {
        let text = "```fig\ntitle = colophon\ncontents = [docs/design.md]\n```\n# Body\n";
        let doc = Document::parse("README.md", text).unwrap();
        assert_eq!(
            doc.meta.get("title").and_then(Value::as_str),
            Some("colophon")
        );
        assert_eq!(doc.body, "# Body\n");
        assert_eq!(doc.embed, Some(EmbedType::FrontmatterFig));
        assert!(doc.has_meta());
    }

    #[test]
    fn parses_json_frontmatter() {
        let text = ";;;\n{\"title\": \"Root\"}\n;;;\nbody\n";
        let doc = Document::parse("note.md", text).unwrap();
        assert_eq!(doc.meta.get("title").and_then(Value::as_str), Some("Root"));
        assert_eq!(doc.embed, Some(EmbedType::FrontmatterJson));
    }

    #[test]
    fn parses_yaml_endmatter() {
        let text = "# Body first\n```endmatter\ntitle: Tail\n```\n";
        let doc = Document::parse("note.md", text).unwrap();
        assert_eq!(doc.meta.get("title").and_then(Value::as_str), Some("Tail"));
        assert_eq!(doc.body, "# Body first\n");
        assert_eq!(doc.embed, Some(EmbedType::EndmatterYaml));
    }

    #[test]
    fn no_frontmatter_is_all_body() {
        let doc = Document::parse("note.md", "# Just a note\n").unwrap();
        assert!(doc.meta.is_null());
        assert_eq!(doc.body, "# Just a note\n");
        assert_eq!(doc.embed, None);
        assert!(!doc.has_meta());
    }

    #[test]
    fn unterminated_fence_is_not_frontmatter() {
        let text = "---\ntitle: oops\nno closing fence\n";
        let doc = Document::parse("x.md", text).unwrap();
        assert!(doc.meta.is_null());
        assert_eq!(doc.body, text);
        assert_eq!(doc.embed, None);
    }

    #[test]
    fn crlf_fences_are_handled() {
        let text = "---\r\ntitle: Root\r\n---\r\nbody\r\n";
        let doc = Document::parse("x.md", text).unwrap();
        assert_eq!(doc.embed, Some(EmbedType::FrontmatterYaml));
        assert_eq!(doc.body, "body\r\n");
        // Exact scalar — fig ≥ 2.1.1 treats \r\n as a single line break.
        assert_eq!(doc.meta.get("title").and_then(Value::as_str), Some("Root"));
    }
}
