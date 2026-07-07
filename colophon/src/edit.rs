//! Format-preserving edits to a document's embedded metadata.
//!
//! These are text → text: they detect the document's embed archetype
//! (`fig::detect`), open it with fig's comment-preserving [`fig::Embed`]
//! editor, apply the edit, and re-render — only the changed node's bytes move.
//! Comments, key order, fence style, and the body are untouched, and a
//! ```` ```fig ```` block is never rewritten as YAML.
//!
//! The workspace mutation ops ([`crate::workspace::Workspace`]) build on the
//! same editor; these free functions are the single-document surface (the
//! CLI's `set`/`unset`).

use fig::{Embed, EmbedType, Segment};

use crate::error::{Error, Result};

/// Parse a dotted key path (`a.b.0.c`) into fig path segments. An all-digit
/// segment indexes a sequence; anything else names a mapping key.
pub fn key_path(dotted: &str) -> Vec<Segment<'_>> {
    dotted
        .split('.')
        .map(|part| match part.parse::<usize>() {
            Ok(index) => Segment::Index(index),
            Err(_) => Segment::Key(part),
        })
        .collect()
}

/// Interpret a CLI-provided scalar: `true`/`false`, integers, floats, and
/// `null` become their typed values; everything else stays a string.
pub fn infer_scalar(s: &str) -> fig::Value {
    match s {
        "true" => fig::Value::Bool(true),
        "false" => fig::Value::Bool(false),
        "null" | "~" => fig::Value::Null,
        _ => {
            if let Ok(i) = s.parse::<i64>() {
                fig::Value::Int(i)
            } else if let Ok(f) = s.parse::<f64>() {
                fig::Value::Float(f)
            } else {
                fig::Value::Str(s.to_string())
            }
        }
    }
}

/// Upsert `dotted` to `value` in `text`'s metadata block, creating the block
/// (YAML frontmatter by default) when the document has none. Returns the full
/// re-rendered document text.
pub fn set_in_text(text: &str, dotted: &str, value: fig::Value) -> Result<String> {
    let kind = fig::detect(text).unwrap_or(EmbedType::FrontmatterYaml);
    let mut embed = Embed::open_or_init(text.as_bytes(), kind)?;
    let path = key_path(dotted);
    match path.last() {
        // fig's `set` upserts a trailing *key*; an index-terminated path is a
        // pure replacement (there is no "insert at absent index" to upsert).
        Some(Segment::Index(_)) => embed.replace_value(&path, value)?,
        _ => embed.set_value(&path, value)?,
    }
    Ok(embed.render()?.to_string())
}

/// Delete the entry at `dotted` from `text`'s metadata block. Returns the full
/// re-rendered document text. Errors when the document has no metadata block
/// or the path does not exist.
pub fn unset_in_text(text: &str, dotted: &str) -> Result<String> {
    let kind = fig::detect(text)
        .ok_or_else(|| Error::Structure("document has no embedded metadata block".into()))?;
    let mut embed = Embed::open(text.as_bytes(), kind)?;
    embed.delete(&key_path(dotted))?;
    Ok(embed.render()?.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_preserves_comments_and_format() {
        let text = "---\n# keep me\ntitle: Old\n---\nbody\n";
        let out = set_in_text(text, "title", infer_scalar("New")).unwrap();
        assert_eq!(out, "---\n# keep me\ntitle: New\n---\nbody\n");
    }

    #[test]
    fn set_in_a_fig_block_stays_fig() {
        let text = "```fig\ntitle = colophon\n```\nbody\n";
        let out = set_in_text(text, "title", infer_scalar("renamed")).unwrap();
        assert!(out.starts_with("```fig\n"), "fence preserved: {out}");
        assert!(out.contains("title = renamed"), "fig dialect preserved: {out}");
        assert!(out.ends_with("```\nbody\n"));
    }

    #[test]
    fn set_creates_a_block_when_none_exists() {
        let out = set_in_text("just a body\n", "title", infer_scalar("T")).unwrap();
        assert!(out.starts_with("---\ntitle: T\n---\n"), "{out}");
        assert!(out.ends_with("just a body\n"));
    }

    #[test]
    fn unset_removes_only_the_named_key() {
        let text = "---\ntitle: T\ndraft: true\n---\nbody\n";
        let out = unset_in_text(text, "draft").unwrap();
        assert_eq!(out, "---\ntitle: T\n---\nbody\n");
        assert!(unset_in_text("no meta\n", "x").is_err());
    }

    #[test]
    fn scalars_are_inferred() {
        assert_eq!(infer_scalar("true"), fig::Value::Bool(true));
        assert_eq!(infer_scalar("42"), fig::Value::Int(42));
        assert_eq!(infer_scalar("4.5"), fig::Value::Float(4.5));
        assert_eq!(infer_scalar("null"), fig::Value::Null);
        assert_eq!(infer_scalar("hello"), fig::Value::Str("hello".into()));
    }

    #[test]
    fn dotted_paths_mix_keys_and_indices() {
        let text = "---\ncontents:\n- a.md\n- b.md\n---\n";
        let out = set_in_text(text, "contents.1", infer_scalar("c.md")).unwrap();
        assert!(out.contains("- a.md\n- c.md"), "{out}");
    }
}
