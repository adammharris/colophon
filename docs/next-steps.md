# Next steps — working notes

Deferred items from the identity / wikilink / link-syntax work, so we don't lose
them. Not curated design (that's `DESIGN.md`); this is a scratch backlog.

## Identity & backlinks

- **Step 4 — gated malformed-id autofix.** The one document-repairing heal: when
  the census finds a malformed `colophon:<id>` near an edge the registry resolves
  uniquely, offer to restore it. Directional invariant: forward links are ground
  truth; the index heals *toward* them; it rewrites a document *only* for a
  dangling id it can resolve from its own record. Everything else: report.

- **Route C — persist the backlink map.** Where "id-backlink registration"
  finally has a home. Store the census-derived backlink map in the index's
  *derived* section (structurally separate from the authoritative `id → path`
  registry, per DESIGN §5), plus a `Reconciled` report (backlink added/dropped,
  out-of-band id-link registered). The census is its self-heal.

- **Frontmatter id-shadow (DESIGN §5 escape hatch).** Stamp a forward id-link's
  id into the *source's* frontmatter so the forward-link truth is complete in the
  nodes: backlinks become fully derivable, the §4 out-of-band hazard becomes
  recoverable, and self-healing goes total — no central authoritative residue.
  The thesis-aligned alternative to Route C.

- **Authoring `[[colophon:id]]` wikilinks.** The write side of the original
  idea #2: mint via `Trigger::Link`, drop the target into body prose. Closes the
  loop — the whole census/rename/backlink stack was built to support this.

## Mutation

- **`delete` autofix.** `delete` now *diagnoses* inbound danglers; optionally
  offer to remove/rewrite them (careful — a link records intent).

## Link-syntax layer (this session's thread)

- **Round-trip faithfulness on `mv`.** Parsing now accepts angle-bracketed and
  workspace-absolute targets; the *write* path (rename rewrite) should preserve
  the convention — keep an absolute link absolute, keep bracket style — instead
  of always emitting a relative path. Needs the `Link` to remember "was
  absolute" (bracketing already round-trips via the space heuristic).
- **Extract a link-syntax crate — later, not now.** The "fig for links" idea is
  right, but publish only once a second real consumer (diaryx's migration onto
  colophon, per DESIGN §9) validates the API. For now it lives in `link.rs` as
  colophon's link-syntax layer.
