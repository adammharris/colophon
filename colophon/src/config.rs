//! Workspace configuration — the typed policy a standalone/CLI workspace reads
//! from its **config document** (the `config`-relation target from the root,
//! DESIGN §6's reachability move applied to policy) and from its root's
//! `colophon:` frontmatter block.
//!
//! Programmatic embedders never need this: they configure the [`Workspace`]
//! directly through the builder (`.link_style`, `.identity`, …), which is why
//! the type-level identity/index choice lives there. `WorkspaceConfig` is the
//! **data** shape that lets a workspace configure *itself* — so the same tool
//! serves a Diaryx-style vault and an Obsidian-style one purely by what the
//! config declares:
//!
//! - [`WorkspaceConfig::paths_only`] — path links, identity off (pure paths).
//! - [`WorkspaceConfig::stable_ids`] — stable IDs minted lazily (registry +
//!   backlinks), portable links for the path-based parts.
//!
//! The vocabulary (`docs/config-vocab.md`) is one namespace of keys with two
//! homes: nested under `colophon:` in the root's frontmatter (the description
//! home) or at the top level of the dedicated config document (the policy home).
//! [`apply`](WorkspaceConfig::apply) reads either shape; unset keys keep their
//! default, and layering root block then config document gives the precedence
//! *config document > root `colophon:` block > default*.
//!
//! [`Workspace`]: crate::workspace::Workspace

use std::collections::BTreeMap;

use crate::content::ContentFormat;
use crate::document::EmbedStyle;
use crate::identity::Registration;
use crate::link::{Addressing, LinkStyle, Notation, PathStyle, ReferenceStyle};
use crate::meta::{Mapping, Value};

/// The config-vocabulary version stamped as `spec` and recognized on read — a
/// marker so a foreign tool (or a future colophon) knows which vocabulary it is
/// looking at. Bumped only on an incompatible reshape.
pub const SPEC_VERSION: i64 = 1;

/// The root-frontmatter key under which workspace policy is nested. A root
/// document's frontmatter mixes structural links, identity, and user-owned
/// fields with the occasional policy setting; nesting policy under this one key
/// keeps the two apart, so config is unambiguous to read *and* to lint, and an
/// unrecognized *sibling* is never mistaken for a misspelled setting. The
/// dedicated config document needs no such wrapper — the whole document is policy
/// (`docs/config-vocab.md`, "The two homes").
pub const ROOT_CONFIG_KEY: &str = "colophon";

/// A per-relation reference-style override, as declared in a config's
/// `relations` block. Each axis is optional and inherits the workspace default
/// ([`WorkspaceConfig::reference_style`]) when absent — so a block need only name
/// the axes it changes. This is the config form of
/// [`Relation::style`](crate::relation::Relation::style), and what lets links
/// going "down" (`contents`) differ from links going "up" (`part_of`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RelationStyleConfig {
    /// The notation override (`markdown` / `wikilink` / `bare`).
    pub notation: Option<Notation>,
    /// The path-resolution override (`root` / `relative` / `canonical`).
    pub path_style: Option<PathStyle>,
    /// The addressing override (`path` / `id` / `alias`).
    pub target: Option<Addressing>,
    /// The `id`-wikilink label override.
    pub label: Option<bool>,
}

/// Where a document's stable ID is persisted — the identity-storage axis
/// (DESIGN §5). Orthogonal to *when* an ID is minted ([`Registration`]) and to
/// how references are spelled; this is purely the ID's *home*.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum IdStorage {
    /// **Registry only** (`registry`): IDs live solely in the registry document —
    /// authoritative, non-derivable, resolved by direct lookup. The cleanest
    /// documents (no `id` clutter), but identity does not travel with a file.
    Registry,
    /// **Frontmatter + registry** (`both`, the default): each document also
    /// carries its own ID in an `id` frontmatter field (a portable, self-describing
    /// shadow), and the registry is retained as a rebuildable cache + tombstone
    /// ledger. The ID travels with the file across copies and out-of-band moves.
    #[default]
    Frontmatter,
    /// **Frontmatter only** (`frontmatter`): the `id` field is the sole home; no
    /// registry document is written and resolution rebuilds the id→path map by
    /// scanning frontmatter. Maximally self-describing, but it forfeits tombstones
    /// (a deleted file takes its ID with it), so an ID can in principle be reminted.
    FrontmatterOnly,
}

impl IdStorage {
    /// Whether this mode writes the ID into each document's `id` frontmatter.
    pub fn stamps_frontmatter(self) -> bool {
        matches!(self, IdStorage::Frontmatter | IdStorage::FrontmatterOnly)
    }

    /// Whether this mode keeps a registry document (the authoritative store, or —
    /// under [`Frontmatter`](IdStorage::Frontmatter) — a rebuildable cache).
    pub fn keeps_registry(self) -> bool {
        matches!(self, IdStorage::Registry | IdStorage::Frontmatter)
    }

    /// Parse the `id_storage` config spelling; unknown → `None`. `both` is the
    /// frontmatter+registry default; `frontmatter` is the registry-less mode.
    pub fn from_config_str(value: &str) -> Option<Self> {
        match value {
            "registry" => Some(Self::Registry),
            "both" => Some(Self::Frontmatter),
            "frontmatter" => Some(Self::FrontmatterOnly),
            _ => None,
        }
    }

    /// The `id_storage` config spelling.
    pub fn as_config_str(self) -> &'static str {
        match self {
            Self::Registry => "registry",
            Self::Frontmatter => "both",
            Self::FrontmatterOnly => "frontmatter",
        }
    }
}

/// How far content-checksum (fixity) coverage extends — the archival integrity
/// axis. Orthogonal to the identity and link axes; this is purely about
/// detecting bit-rot in stored bytes.
///
/// The tiers exist because fixity means different things for different content.
/// An **attachment** is never edited, so a change to its bytes is unambiguously
/// corruption — safe to checksum by default, with no friction. A **document
/// body** *is* edited, and a legitimate external edit is indistinguishable from
/// rot to a checker, so hashing bodies is opt-in and best paired with
/// `colophon edit` (which restamps on save). Frontmatter is never hashed: it is
/// small, structured, edited constantly by colophon's own link maintenance, and
/// its corruption already surfaces as parse or link findings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Fixity {
    /// No content checksums are recorded or verified (`off`).
    Off,
    /// **Attachments only** (`attachments`, the default): each attachment sidecar
    /// records a `content_hash` of its payload, and `check` verifies it.
    /// Unambiguous — a payload's bytes changing is always corruption — so there is
    /// no edit friction and nothing to opt out of per document.
    #[default]
    Payloads,
    /// **Attachments and document bodies** (`all`): additionally, each document
    /// records a `content_hash` of its *body* (never its frontmatter). The
    /// archival-grade tier; because a body is editable, pair it with
    /// `colophon edit` so a body change restamps the hash, and treat an
    /// out-of-band edit as a `check` finding to re-bless rather than a hard error.
    Full,
}

impl Fixity {
    /// Whether attachment payloads are checksummed (true for every tier but off).
    pub fn covers_payloads(self) -> bool {
        matches!(self, Fixity::Payloads | Fixity::Full)
    }

    /// Whether document bodies are checksummed (only the `all` tier).
    pub fn covers_bodies(self) -> bool {
        matches!(self, Fixity::Full)
    }

    /// Parse the `fixity` config spelling; unknown → `None`.
    pub fn from_config_str(value: &str) -> Option<Self> {
        match value {
            "off" => Some(Self::Off),
            "attachments" => Some(Self::Payloads),
            "all" => Some(Self::Full),
            _ => None,
        }
    }

    /// The `fixity` config spelling.
    pub fn as_config_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Payloads => "attachments",
            Self::Full => "all",
        }
    }
}

/// The workspace-wide policy a config declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceConfig {
    /// When a document earns a stable ID — the identity registration triggers.
    pub identity: Registration,
    /// The default reference **notation** (`markdown` / `wikilink` / `bare`).
    /// Overridden per relation by [`Relation::style`](crate::relation::Relation::style).
    pub notation: Notation,
    /// The default **path resolution** for path targets (`root` / `relative` /
    /// `canonical`). Ignored for id/alias targets.
    pub path_style: PathStyle,
    /// The default reference **addressing** (`path` / `id` / `alias`).
    pub reference_target: Addressing,
    /// Whether an id/alias reference carries a `|Title` label.
    pub reference_label: bool,
    /// Per-relation reference-style overrides, keyed by relation name — the
    /// config form of [`Relation::style`](crate::relation::Relation::style).
    /// Each entry overlays the workspace default for that relation only, letting
    /// `contents` (down) and `part_of` (up) carry different styles. Empty means
    /// every relation inherits the default. Resolve with
    /// [`resolved_relation_styles`](Self::resolved_relation_styles).
    pub relation_styles: BTreeMap<String, RelationStyleConfig>,
    /// Where a document's stable ID is persisted — registry, frontmatter shadow,
    /// or both (DESIGN §5). Independent of the `identity` trigger.
    pub id_storage: IdStorage,
    /// The metadata format new documents get when they inherit no parent block
    /// — a *default* for authoring, never a workspace constraint (§7).
    pub default_embed_format: fig::Format,
    /// How that metadata is *embedded* — delimiters, a fenced code block, an
    /// HTML island, or a separate sidecar. Together with `default_embed_format`
    /// it selects the carrier a fresh root/document is authored in; recorded so
    /// the workspace is self-describing about its embedding convention. Like
    /// `default_embed_format`, an authoring default rather than a constraint:
    /// existing documents keep whatever carrier they already have.
    pub embed_style: EmbedStyle,
    /// The body-prose grammar the workspace is authored in (Markdown/Djot/HTML)
    /// — the format `render` and code-aware link scanning assume, and the
    /// intended default for new documents.
    pub content_format: ContentFormat,
    /// Whether a `delete` moves the document to the **recycle bin** (recoverable)
    /// rather than destroying it. On by default — the safe posture for archival
    /// use, where a deletion should never be silently unrecoverable — and opt-out
    /// per workspace for those who genuinely want a hard delete as the default.
    pub recycle_bin: bool,
    /// How far content-checksum (fixity) coverage extends — attachments only (the
    /// default), attachments plus document bodies, or off.
    pub fixity: Fixity,
    /// The frontmatter field `colophon edit` stamps with the current time when a
    /// document's content changes — the machine-maintained "last updated" field.
    /// Empty (the default) disables it. The *name* is yours (`updated`,
    /// `modified`, `lastmod`); the *value* is always machine-standard (RFC 3339
    /// UTC), because colophon reads it back to know when to rewrite it. A
    /// human-friendly date is a *different*, user-owned field colophon never
    /// touches (see DESIGN §2, "does colophon read it back?").
    pub updated: String,
}

impl Default for WorkspaceConfig {
    /// The standalone default: portable markdown-root path links, identity
    /// available lazily (IDs minted only on a durable link-by-id or publish, §4),
    /// and path addressing (id-linking is opt-in).
    fn default() -> Self {
        Self {
            identity: Registration::LAZY,
            notation: Notation::Markdown,
            path_style: PathStyle::Root,
            reference_target: Addressing::Path,
            reference_label: false,
            relation_styles: BTreeMap::new(),
            id_storage: IdStorage::Frontmatter,
            default_embed_format: fig::Format::Yaml,
            embed_style: EmbedStyle::Delimited,
            content_format: ContentFormat::Markdown,
            recycle_bin: true,
            fixity: Fixity::Payloads,
            updated: String::new(),
        }
    }
}

impl WorkspaceConfig {
    /// Diaryx-style: path links, no identity — nothing mints an ID, so the
    /// workspace is addressed purely by path (the Adam's-Archive shape).
    pub fn paths_only() -> Self {
        Self {
            identity: Registration::OFF,
            id_storage: IdStorage::Registry,
            ..Self::default()
        }
    }

    /// Obsidian-style: stable IDs minted lazily (link-by-id or publish), and
    /// colophon authors structural links *by* id — so a move rewrites nothing,
    /// the registry keeps them resolving. Portable path links for the rest.
    pub fn stable_ids() -> Self {
        Self {
            identity: Registration::LAZY,
            reference_target: Addressing::Id,
            id_storage: IdStorage::Registry,
            ..Self::default()
        }
    }

    /// The fused path [`LinkStyle`] this config's notation + path resolution
    /// select — what the [`Workspace`](crate::workspace::Workspace) builder's
    /// `link_style` expects for authoring structural path links.
    pub fn link_format(&self) -> LinkStyle {
        LinkStyle::from_axes(self.notation, self.path_style)
    }

    /// The effective workspace-default [`ReferenceStyle`] — the fallback for any
    /// relation without its own override, composed from the four reference axes.
    pub fn reference_style(&self) -> ReferenceStyle {
        ReferenceStyle {
            wrapper: self.notation.wrapper(),
            addressing: self.reference_target,
            label: self.reference_label,
            path_style: LinkStyle::from_axes(self.notation, self.path_style),
        }
        .normalized()
    }

    /// The declared per-relation overrides resolved to full [`ReferenceStyle`]s,
    /// each partial overlaid on the workspace default ([`reference_style`]) and
    /// normalized. Feed the result to
    /// [`RelationSet::with_styles`](crate::relation::RelationSet::with_styles) to
    /// build the workspace's relation vocabulary from a config. Empty when no
    /// relation declares an override — every relation then inherits the default.
    ///
    /// [`reference_style`]: Self::reference_style
    pub fn resolved_relation_styles(&self) -> BTreeMap<String, ReferenceStyle> {
        let base = self.reference_style();
        let base_notation = Notation::from_wrapper(base.wrapper, base.path_style);
        let base_path = base.path_style.axes().1;
        self.relation_styles
            .iter()
            .map(|(name, over)| {
                let notation = over.notation.unwrap_or(base_notation);
                let path = over.path_style.unwrap_or(base_path);
                let style = ReferenceStyle {
                    wrapper: notation.wrapper(),
                    addressing: over.target.unwrap_or(base.addressing),
                    label: over.label.unwrap_or(base.label),
                    path_style: LinkStyle::from_axes(notation, path),
                }
                .normalized();
                (name.clone(), style)
            })
            .collect()
    }

    /// Whether a *mutation* under this config could mint a new stable ID — so a
    /// caller that will land one must bootstrap a registry document *first*
    /// (before the change set that would otherwise strand the id→path map with no
    /// home). Two ways an op mints: an **eager** identity policy stamps every
    /// created document, and any **id-registering reference style** (the workspace
    /// default, or a single relation's override — e.g. `part_of: id` in a split)
    /// registers a link's target when a `link` fires.
    ///
    /// This is the single home for a judgment the CLI previously recomputed at
    /// every mutation command (`new`, `attach`, `mv --in`, `reparent`,
    /// `duplicate`, `init`'s adoption pass), each an identical copy of the same
    /// three-line `link_registers && fires_on(Link) || fires_on(Create)` — the
    /// kind of duplicated policy that drifts silently. It lives here because every
    /// term it needs is a fact about the config.
    pub fn mints_on_mutation(&self) -> bool {
        let link_registers = self.reference_style().registers()
            || self
                .resolved_relation_styles()
                .values()
                .any(|s| s.registers());
        (link_registers && self.identity.fires_on(crate::identity::Trigger::Link))
            || self.identity.fires_on(crate::identity::Trigger::Create)
    }

    /// Overlay the recognized keys present in `meta` onto this config; absent
    /// keys keep their current value. `meta` is either a root's `colophon:` block
    /// or a config document's top-level mapping — the same nested shape. Apply the
    /// root block first, then the config document, so the config document wins.
    pub fn apply(&mut self, meta: &Value) {
        if let Some(v) = meta
            .get("content_format")
            .and_then(Value::as_str)
            .and_then(ContentFormat::from_config_str)
        {
            self.content_format = v;
        }
        if let Some(md) = meta.get("metadata") {
            if let Some(v) = md
                .get("format")
                .and_then(Value::as_str)
                .and_then(format_from_str)
            {
                self.default_embed_format = v;
            }
            if let Some(v) = md
                .get("embed")
                .and_then(Value::as_str)
                .and_then(EmbedStyle::from_config_str)
            {
                self.embed_style = v;
            }
        }
        if let Some(rf) = meta.get("references") {
            if let Some(v) = rf
                .get("notation")
                .and_then(Value::as_str)
                .and_then(Notation::from_config_str)
            {
                self.notation = v;
            }
            if let Some(v) = rf
                .get("path_style")
                .and_then(Value::as_str)
                .and_then(PathStyle::from_config_str)
            {
                self.path_style = v;
            }
            if let Some(v) = rf
                .get("target")
                .and_then(Value::as_str)
                .and_then(Addressing::from_config_str)
            {
                self.reference_target = v;
            }
            if let Some(v) = rf.get("label").and_then(Value::as_bool) {
                self.reference_label = v;
            }
        }
        // Per-relation overrides: `relations: { <name>: { notation, target, … } }`.
        if let Some(relations) = meta.get("relations").and_then(Value::as_mapping) {
            for (name, spec) in relations {
                let entry = self.relation_styles.entry(name.clone()).or_default();
                if let Some(v) = spec
                    .get("notation")
                    .and_then(Value::as_str)
                    .and_then(Notation::from_config_str)
                {
                    entry.notation = Some(v);
                }
                if let Some(v) = spec
                    .get("path_style")
                    .and_then(Value::as_str)
                    .and_then(PathStyle::from_config_str)
                {
                    entry.path_style = Some(v);
                }
                if let Some(v) = spec
                    .get("target")
                    .and_then(Value::as_str)
                    .and_then(Addressing::from_config_str)
                {
                    entry.target = Some(v);
                }
                if let Some(v) = spec.get("label").and_then(Value::as_bool) {
                    entry.label = Some(v);
                }
            }
        }
        if let Some(v) = meta
            .get("id_storage")
            .and_then(Value::as_str)
            .and_then(IdStorage::from_config_str)
        {
            self.id_storage = v;
        }
        if let Some(v) = meta.get("updated").and_then(Value::as_str) {
            self.updated = v.to_string();
        }
        if let Some(v) = meta
            .get("identity")
            .and_then(Value::as_str)
            .and_then(registration_from_str)
        {
            self.identity = v;
        }
        if let Some(v) = meta
            .get("fixity")
            .and_then(Value::as_str)
            .and_then(Fixity::from_config_str)
        {
            self.fixity = v;
        }
        if let Some(v) = meta.get("recycle_bin").and_then(Value::as_bool) {
            self.recycle_bin = v;
        }
    }

    /// A fresh config with `meta`'s recognized keys applied over the defaults.
    pub fn from_meta(meta: &Value) -> Self {
        let mut config = Self::default();
        config.apply(meta);
        config
    }

    /// This config as config-document metadata keys (the nested vocabulary,
    /// `docs/config-vocab.md`). Emitted at the top level of the config document;
    /// the same mapping nests under `colophon:` in a root's frontmatter.
    pub fn to_mapping(&self) -> Mapping {
        let mut map = Mapping::new();
        map.insert("spec".into(), Value::Int(SPEC_VERSION));
        map.insert(
            "content_format".into(),
            Value::String(self.content_format.as_config_str().into()),
        );

        let mut metadata = Mapping::new();
        metadata.insert(
            "format".into(),
            Value::String(format_str(self.default_embed_format).into()),
        );
        metadata.insert(
            "embed".into(),
            Value::String(self.embed_style.as_config_str().into()),
        );
        map.insert("metadata".into(), Value::Mapping(metadata));

        let mut references = Mapping::new();
        references.insert(
            "notation".into(),
            Value::String(self.notation.as_config_str().into()),
        );
        references.insert(
            "path_style".into(),
            Value::String(self.path_style.as_config_str().into()),
        );
        references.insert(
            "target".into(),
            Value::String(self.reference_target.as_config_str().into()),
        );
        references.insert("label".into(), Value::Bool(self.reference_label));
        map.insert("references".into(), Value::Mapping(references));

        if !self.relation_styles.is_empty() {
            let mut relations = Mapping::new();
            for (name, over) in &self.relation_styles {
                let mut spec = Mapping::new();
                if let Some(n) = over.notation {
                    spec.insert("notation".into(), Value::String(n.as_config_str().into()));
                }
                if let Some(p) = over.path_style {
                    spec.insert("path_style".into(), Value::String(p.as_config_str().into()));
                }
                if let Some(t) = over.target {
                    spec.insert("target".into(), Value::String(t.as_config_str().into()));
                }
                if let Some(l) = over.label {
                    spec.insert("label".into(), Value::Bool(l));
                }
                relations.insert(name.clone(), Value::Mapping(spec));
            }
            map.insert("relations".into(), Value::Mapping(relations));
        }

        map.insert(
            "id_storage".into(),
            Value::String(self.id_storage.as_config_str().into()),
        );
        map.insert("updated".into(), Value::String(self.updated.clone()));
        map.insert(
            "identity".into(),
            Value::String(registration_str(self.identity).into()),
        );
        map.insert(
            "fixity".into(),
            Value::String(self.fixity.as_config_str().into()),
        );
        map.insert("recycle_bin".into(), Value::Bool(self.recycle_bin));
        map
    }
}

// ── Config linting (`docs/config-vocab.md`, "Linting") ──────────────────────

/// A key in a config surface that [`WorkspaceConfig::apply`] would silently
/// ignore — surfaced so a setting that never takes effect becomes visible rather
/// than staying invisible. `apply` keeps the current value whenever a key is
/// unrecognized or its value fails to parse; that robustness is what makes a
/// typo (`notaton`) or a bad value (`fixity: alll`) vanish without a word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigIssue {
    /// The offending key, dotted from the block root (`references.notation`).
    pub key: String,
    /// What is wrong with it.
    pub kind: ConfigIssueKind,
}

/// The two ways a config key goes unread. See [`ConfigIssue`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigIssueKind {
    /// `key` is not a recognized axis but closely resembles `suggestion` — almost
    /// certainly a misspelling. An unrecognized key that resembles *no* axis at
    /// its level is deliberately **not** reported: a config surface can carry
    /// user-owned fields colophon never reads (DESIGN §2), so flagging every
    /// unknown key would be noise.
    UnknownKey { suggestion: String },
    /// `key` is a recognized axis but `value` is not a spelling colophon
    /// understands, so `apply` kept the default. `expected` lists the accepted
    /// spellings (advisory help; mirrors the axis's parser).
    InvalidValue {
        value: String,
        expected: Vec<String>,
    },
}

/// Top-level config keys (block names + scalar axes + the `spec` marker).
const TOP_KEYS: &[&str] = &[
    "spec",
    "content_format",
    "metadata",
    "references",
    "relations",
    "id_storage",
    "updated",
    "identity",
    "fixity",
    "recycle_bin",
];
/// Keys inside the `metadata:` block.
const METADATA_KEYS: &[&str] = &["format", "embed"];
/// Keys inside the `references:` block and each `relations.<name>` entry.
const REFERENCE_KEYS: &[&str] = &["notation", "path_style", "target", "label"];

/// If `meta` declares a `spec` newer than [`SPEC_VERSION`] — the version this
/// build understands — the declared version. The signal that colophon may be
/// silently ignoring settings a newer colophon wrote. `None` when `spec` is
/// absent, not an integer, or within range. Shared by `check` (a
/// `Finding::ConfigSpecAhead`) and the CLI's proactive config warning, so the
/// version comparison lives in one place.
pub fn spec_ahead(meta: &Value) -> Option<i64> {
    match meta.get("spec") {
        Some(Value::Int(v)) if *v > SPEC_VERSION => Some(*v),
        _ => None,
    }
}

/// Diagnose a config surface (a root's `colophon:` block or a config document's
/// top-level mapping): one [`ConfigIssue`] per key `apply` would silently ignore.
/// Recognized keys are checked for a value colophon can parse; unrecognized keys
/// are reported only when they closely resemble a real axis at their level (a
/// likely typo). Returns empty for a clean config.
pub fn diagnose(meta: &Value) -> Vec<ConfigIssue> {
    let mut issues = Vec::new();
    let Some(map) = meta.as_mapping() else {
        return issues;
    };
    for (key, value) in map {
        match key.as_str() {
            "spec" => {} // version marker — not a policy axis
            "content_format" => {
                enum_axis(
                    &mut issues,
                    key,
                    value,
                    |s| ContentFormat::from_config_str(s).is_some(),
                    &["markdown", "djot", "html"],
                );
            }
            "id_storage" => {
                enum_axis(
                    &mut issues,
                    key,
                    value,
                    |s| IdStorage::from_config_str(s).is_some(),
                    &["registry", "frontmatter", "both"],
                );
            }
            "identity" => {
                enum_axis(
                    &mut issues,
                    key,
                    value,
                    |s| registration_from_str(s).is_some(),
                    &["none", "lazy", "eager"],
                );
            }
            "fixity" => {
                enum_axis(
                    &mut issues,
                    key,
                    value,
                    |s| Fixity::from_config_str(s).is_some(),
                    &["off", "attachments", "all"],
                );
            }
            "recycle_bin" => bool_axis(&mut issues, key, value),
            "updated" => {} // free-form field name
            "metadata" => diagnose_metadata(&mut issues, value),
            "references" => diagnose_reference_block(&mut issues, "references", value),
            "relations" => diagnose_relations(&mut issues, value),
            other => {
                if let Some(suggestion) = nearest(other, TOP_KEYS) {
                    issues.push(unknown(key.clone(), suggestion));
                }
            }
        }
    }
    issues
}

/// Diagnose the `metadata:` block.
fn diagnose_metadata(issues: &mut Vec<ConfigIssue>, value: &Value) {
    let Some(map) = value.as_mapping() else {
        return block_shape_issue(issues, "metadata", value);
    };
    for (key, v) in map {
        let dotted = format!("metadata.{key}");
        match key.as_str() {
            "format" => enum_axis(
                issues,
                &dotted,
                v,
                |s| format_from_str(s).is_some(),
                &embed_format_spellings(),
            ),
            "embed" => enum_axis(
                issues,
                &dotted,
                v,
                |s| EmbedStyle::from_config_str(s).is_some(),
                &[
                    "delimited",
                    "code_block",
                    "html_script",
                    "html_code",
                    "separate",
                ],
            ),
            other => {
                if let Some(sug) = nearest(other, METADATA_KEYS) {
                    issues.push(unknown(dotted, format!("metadata.{sug}")));
                }
            }
        }
    }
}

/// Diagnose a `references:`-shaped block (the workspace default or a
/// `relations.<name>` entry), `prefix` dotting the reported keys.
fn diagnose_reference_block(issues: &mut Vec<ConfigIssue>, prefix: &str, value: &Value) {
    let Some(map) = value.as_mapping() else {
        return block_shape_issue(issues, prefix, value);
    };
    for (key, v) in map {
        let dotted = format!("{prefix}.{key}");
        match key.as_str() {
            "notation" => enum_axis(
                issues,
                &dotted,
                v,
                |s| Notation::from_config_str(s).is_some(),
                &["markdown", "wikilink", "bare"],
            ),
            "path_style" => enum_axis(
                issues,
                &dotted,
                v,
                |s| PathStyle::from_config_str(s).is_some(),
                &["root", "relative", "canonical"],
            ),
            "target" => enum_axis(
                issues,
                &dotted,
                v,
                |s| Addressing::from_config_str(s).is_some(),
                &["path", "id", "alias"],
            ),
            "label" => bool_axis(issues, &dotted, v),
            other => {
                if let Some(sug) = nearest(other, REFERENCE_KEYS) {
                    issues.push(unknown(dotted, format!("{prefix}.{sug}")));
                }
            }
        }
    }
}

/// Diagnose the `relations:` block — a mapping of relation name to a
/// reference-shaped override.
fn diagnose_relations(issues: &mut Vec<ConfigIssue>, value: &Value) {
    let Some(map) = value.as_mapping() else {
        return block_shape_issue(issues, "relations", value);
    };
    for (name, spec) in map {
        diagnose_reference_block(issues, &format!("relations.{name}"), spec);
    }
}

/// Flag a block key whose value is not a mapping (e.g. `references: markdown`).
fn block_shape_issue(issues: &mut Vec<ConfigIssue>, key: &str, value: &Value) {
    issues.push(ConfigIssue {
        key: key.to_string(),
        kind: ConfigIssueKind::InvalidValue {
            value: value_summary(value),
            expected: vec!["a block of keys".into()],
        },
    });
}

/// Check an enum-valued axis, pushing an `InvalidValue` (with the accepted
/// spellings) when the written value does not parse.
fn enum_axis(
    issues: &mut Vec<ConfigIssue>,
    key: &str,
    value: &Value,
    parses: impl Fn(&str) -> bool,
    expected: &[&str],
) {
    if !value.as_str().is_some_and(parses) {
        issues.push(ConfigIssue {
            key: key.to_string(),
            kind: ConfigIssueKind::InvalidValue {
                value: value_summary(value),
                expected: expected.iter().map(|s| s.to_string()).collect(),
            },
        });
    }
}

/// Check a bool-valued axis.
fn bool_axis(issues: &mut Vec<ConfigIssue>, key: &str, value: &Value) {
    if value.as_bool().is_none() {
        issues.push(ConfigIssue {
            key: key.to_string(),
            kind: ConfigIssueKind::InvalidValue {
                value: value_summary(value),
                expected: vec!["true".into(), "false".into()],
            },
        });
    }
}

fn unknown(key: String, suggestion: String) -> ConfigIssue {
    ConfigIssue {
        key,
        kind: ConfigIssueKind::UnknownKey { suggestion },
    }
}

/// The `metadata.format` spellings compiled into this build (yaml is always
/// available; the rest are feature-gated, matching [`format_from_str`]).
fn embed_format_spellings() -> Vec<&'static str> {
    // `mut` is used only when a format feature below is compiled in.
    #[allow(unused_mut)]
    let mut v = vec!["yaml"];
    #[cfg(feature = "json")]
    v.push("json");
    #[cfg(feature = "toml")]
    v.push("toml");
    #[cfg(feature = "fig-lang")]
    v.push("fig");
    v
}

/// A short, human-readable rendering of a config value for a diagnostic message.
fn value_summary(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        _ => "(non-scalar)".to_string(),
    }
}

/// The recognized key at `candidates` that most resembles `key`, when one is
/// within a small edit distance (a likely typo) — else `None`. Distance is
/// measured case-sensitively so a case-only slip surfaces its canonical spelling.
/// The threshold (2) is deliberately tight: recognized keys are distinctive
/// enough that structural fields (`title`, `part_of`, `id`) and ordinary user
/// fields fall outside it, so they are never mistaken for typos.
fn nearest(key: &str, candidates: &[&str]) -> Option<String> {
    candidates
        .iter()
        .map(|cand| (levenshtein(key, cand), *cand))
        .filter(|(d, _)| (1..=2).contains(d))
        .min_by_key(|(d, _)| *d)
        .map(|(_, cand)| cand.to_string())
}

/// Levenshtein edit distance — the classic two-row dynamic program.
fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for (i, ca) in a.chars().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == *cb { 0 } else { 1 };
            curr[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(curr[j] + 1);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

/// Parse a `metadata.format` config value (`yaml`/`json`/`toml`/`fig`) into a
/// metadata [`fig::Format`], honoring the compiled-in formats — the public form of
/// [`format_from_str`], for callers that name a frontmatter language from outside
/// the config parser (the CLI's `convert … metadata.format …`).
pub fn metadata_format_from_str(value: &str) -> Option<fig::Format> {
    format_from_str(value)
}

/// The `metadata.format` config spelling for a metadata [`fig::Format`] — the
/// public form of [`format_str`], and the inverse of [`metadata_format_from_str`].
pub fn metadata_format_str(format: fig::Format) -> &'static str {
    format_str(format)
}

/// Parse the `metadata.format` config value into a metadata format (only the
/// compiled-in formats are recognized; others → `None`, keeping the default).
fn format_from_str(value: &str) -> Option<fig::Format> {
    match value {
        "yaml" | "yml" => Some(fig::Format::Yaml),
        #[cfg(feature = "json")]
        "json" => Some(fig::Format::Json),
        #[cfg(feature = "toml")]
        "toml" => Some(fig::Format::Toml),
        #[cfg(feature = "fig-lang")]
        "fig" => Some(fig::Format::Fig),
        _ => None,
    }
}

/// The `metadata.format` config spelling for a metadata format.
fn format_str(format: fig::Format) -> &'static str {
    match format {
        #[cfg(feature = "json")]
        fig::Format::Json => "json",
        #[cfg(feature = "toml")]
        fig::Format::Toml => "toml",
        #[cfg(feature = "fig-lang")]
        fig::Format::Fig => "fig",
        _ => "yaml",
    }
}

/// Parse the `identity` config value into a registration trigger set. `none` is
/// the canonical spelling for "identity off" (see `docs/config-vocab.md`), but
/// `off` is accepted as a synonym so the two never diverge: it is the word the
/// CLI's `--identity` flag and every other "off" axis (`fixity: off`) use, and a
/// user who reaches for it must not be told it is invalid.
fn registration_from_str(value: &str) -> Option<Registration> {
    match value {
        "none" | "off" => Some(Registration::OFF),
        "lazy" => Some(Registration::LAZY),
        "eager" => Some(Registration::EAGER),
        _ => None,
    }
}

/// The `identity` config spelling for a registration trigger set. A custom
/// combination (not one of the three presets) is reported as its nearest name.
fn registration_str(registration: Registration) -> &'static str {
    match registration {
        Registration::OFF => "none",
        Registration::EAGER => "eager",
        _ => "lazy",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Trigger;

    /// A config surface as a `Value::Mapping` from `(key, value)` pairs, values
    /// inferred as bools where they parse.
    fn config_doc(pairs: &[(&str, &str)]) -> Value {
        let mut map = Mapping::new();
        for (k, v) in pairs {
            let value = match *v {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                other => Value::String(other.into()),
            };
            map.insert((*k).into(), value);
        }
        Value::Mapping(map)
    }

    #[test]
    fn presets_encode_the_two_styles() {
        // Diaryx: no identity, path addressing. Obsidian: identity + id addressing.
        assert_eq!(WorkspaceConfig::paths_only().identity, Registration::OFF);
        assert_eq!(
            WorkspaceConfig::paths_only().reference_target,
            Addressing::Path
        );
        assert!(
            WorkspaceConfig::stable_ids()
                .identity
                .fires_on(Trigger::Link)
        );
        assert_eq!(
            WorkspaceConfig::stable_ids().reference_target,
            Addressing::Id
        );
    }

    #[test]
    fn round_trips_through_a_nested_mapping() {
        let config = WorkspaceConfig {
            identity: Registration::EAGER,
            notation: Notation::Bare,
            path_style: PathStyle::Canonical,
            reference_target: Addressing::Id,
            reference_label: true,
            relation_styles: BTreeMap::from([
                (
                    "contents".to_string(),
                    RelationStyleConfig {
                        notation: Some(Notation::Wikilink),
                        path_style: None,
                        target: Some(Addressing::Alias),
                        label: None,
                    },
                ),
                (
                    "part_of".to_string(),
                    RelationStyleConfig {
                        notation: Some(Notation::Markdown),
                        path_style: Some(PathStyle::Relative),
                        target: Some(Addressing::Id),
                        label: Some(false),
                    },
                ),
            ]),
            id_storage: IdStorage::Frontmatter,
            default_embed_format: fig::Format::Yaml,
            embed_style: EmbedStyle::CodeBlock,
            content_format: ContentFormat::Djot,
            recycle_bin: false,
            fixity: Fixity::Full,
            updated: "modified".to_string(),
        };
        let back = WorkspaceConfig::from_meta(&Value::Mapping(config.to_mapping()));
        assert_eq!(back, config);
    }

    #[test]
    fn per_relation_styles_resolve_over_the_workspace_default() {
        // The diaryx up≠down example: a workspace default target of `id`, with
        // `contents` (down) overridden to a nominal alias wikilink and `part_of`
        // (up) to a bare markdown id link — each partial overlaying the default.
        let mut cfg = WorkspaceConfig::default();
        cfg.apply(&config_doc_nested(
            &[("target", "id")],
            &[
                ("contents", &[("notation", "wikilink"), ("target", "alias")]),
                ("part_of", &[("target", "id")]),
            ],
        ));

        let styles = cfg.resolved_relation_styles();
        let down = styles.get("contents").expect("contents style");
        assert_eq!(down.wrapper, crate::link::Wrapper::Wikilink);
        assert_eq!(down.addressing, Addressing::Alias);

        let up = styles.get("part_of").expect("part_of style");
        // Inherits the default notation (markdown), keeps its own id target.
        assert_eq!(up.wrapper, crate::link::Wrapper::Markdown);
        assert_eq!(up.addressing, Addressing::Id);
    }

    /// Build a config value with a top-level `references` block and a `relations`
    /// block of per-relation overrides.
    fn config_doc_nested(
        references: &[(&str, &str)],
        relations: &[(&str, &[(&str, &str)])],
    ) -> Value {
        let mut top = Mapping::new();
        let mut refs = Mapping::new();
        for (k, v) in references {
            refs.insert((*k).into(), Value::String((*v).into()));
        }
        top.insert("references".into(), Value::Mapping(refs));
        let mut rels = Mapping::new();
        for (name, axes) in relations {
            let mut spec = Mapping::new();
            for (k, v) in *axes {
                spec.insert((*k).into(), Value::String((*v).into()));
            }
            rels.insert((*name).into(), Value::Mapping(spec));
        }
        top.insert("relations".into(), Value::Mapping(rels));
        Value::Mapping(top)
    }

    #[test]
    fn reference_axes_orthogonalize_notation_and_resolution() {
        // bare + canonical renders a plain workspace-relative path; wikilink wraps.
        let mut cfg = WorkspaceConfig::default();
        let mut refs = Mapping::new();
        refs.insert("notation".into(), Value::String("bare".into()));
        refs.insert("path_style".into(), Value::String("canonical".into()));
        let mut top = Mapping::new();
        top.insert("references".into(), Value::Mapping(refs));
        cfg.apply(&Value::Mapping(top));
        assert_eq!(cfg.link_format(), LinkStyle::PlainCanonical);
        assert_eq!(cfg.notation, Notation::Bare);
        assert_eq!(cfg.path_style, PathStyle::Canonical);
    }

    #[test]
    fn apply_overlays_only_present_keys_so_the_config_document_wins() {
        let mut config = WorkspaceConfig::default();
        // Root block sets only content_format.
        config.apply(&config_doc(&[("content_format", "djot")]));
        assert_eq!(config.content_format, ContentFormat::Djot);
        assert_eq!(config.identity, Registration::LAZY, "identity untouched");
        // The config document then overrides identity; content_format preserved.
        config.apply(&config_doc(&[("identity", "none")]));
        assert_eq!(config.identity, Registration::OFF);
        assert_eq!(config.content_format, ContentFormat::Djot);
    }

    #[test]
    fn diagnose_is_silent_on_a_clean_config_and_on_user_fields() {
        let doc = config_doc(&[
            ("title", "colophon config"),
            ("part_of", "index.md"),
            ("id", "abc123"),
            ("spec", "1"),
            ("identity", "lazy"),
            ("fixity", "all"),
            ("recycle_bin", "false"),
            ("content_format", "djot"),
            ("id_storage", "both"),
            ("author", "someone"),
        ]);
        assert!(diagnose(&doc).is_empty(), "flagged: {:?}", diagnose(&doc));
    }

    #[test]
    fn diagnose_flags_a_misspelled_top_level_key_with_a_suggestion() {
        let issues = diagnose(&config_doc(&[("recyle_bin", "false")]));
        assert_eq!(issues.len(), 1);
        assert_eq!(
            issues[0].kind,
            ConfigIssueKind::UnknownKey {
                suggestion: "recycle_bin".into()
            }
        );
    }

    #[test]
    fn diagnose_flags_bad_values_and_typos_inside_nested_blocks() {
        // references.notaton (typo) + references.target bad value.
        let mut refs = Mapping::new();
        refs.insert("notaton".into(), Value::String("markdown".into()));
        refs.insert("target".into(), Value::String("pointer".into()));
        let mut top = Mapping::new();
        top.insert("references".into(), Value::Mapping(refs));
        let issues = diagnose(&Value::Mapping(top));
        assert!(
            issues.iter().any(|i| i.key == "references.notaton"
                && matches!(&i.kind, ConfigIssueKind::UnknownKey { suggestion } if suggestion == "references.notation")),
            "{issues:?}"
        );
        assert!(
            issues.iter().any(|i| i.key == "references.target"
                && matches!(&i.kind, ConfigIssueKind::InvalidValue { value, .. } if value == "pointer")),
            "{issues:?}"
        );
    }

    #[test]
    fn diagnose_flags_an_unrecognized_value_on_a_real_key() {
        let issues = diagnose(&config_doc(&[("fixity", "alll")]));
        assert_eq!(issues.len(), 1);
        match &issues[0].kind {
            ConfigIssueKind::InvalidValue { value, expected } => {
                assert_eq!(value, "alll");
                assert!(expected.contains(&"all".to_string()), "{expected:?}");
            }
            other => panic!("expected InvalidValue, got {other:?}"),
        }
    }

    #[test]
    fn spec_ahead_fires_only_for_a_newer_spec() {
        assert_eq!(
            spec_ahead(&config_doc(&[("identity", "lazy")])),
            None,
            "absent spec"
        );
        let at = {
            let mut m = Mapping::new();
            m.insert("spec".into(), Value::Int(SPEC_VERSION));
            Value::Mapping(m)
        };
        assert_eq!(spec_ahead(&at), None, "current spec is fine");
        let ahead = {
            let mut m = Mapping::new();
            m.insert("spec".into(), Value::Int(SPEC_VERSION + 1));
            Value::Mapping(m)
        };
        assert_eq!(spec_ahead(&ahead), Some(SPEC_VERSION + 1));
    }

    #[test]
    fn serialized_defaults_and_presets_all_pass_diagnosis() {
        for config in [
            WorkspaceConfig::default(),
            WorkspaceConfig::paths_only(),
            WorkspaceConfig::stable_ids(),
        ] {
            let serialized = Value::Mapping(config.to_mapping());
            assert!(
                diagnose(&serialized).is_empty(),
                "flagged itself: {:?}",
                diagnose(&serialized)
            );
        }
    }
}
