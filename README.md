```fig
title = colophon
author = adammharris
created = 2026-07-06
contents = [[Design](docs/DESIGN.md)]
```

# colophon

A *self-describing plaintext workspace*: a set of documents whose structure lives in the documents' own embedded metadata (frontmatter), not in the filesystem layout or an app-private sidecar folder.

The name is the point. A *colophon* is the note in which a book describes its own making — the type, the paper, the press. A colophon workspace is one you can hand to any tool and it explains itself: follow the links in the metadata and the whole structure unfolds, with a distinguished root that describes the whole.

## Layout

- **`colophon/`** — the library. Documents, relations, identity, and the workspace seam.
- **`colophon-cli/`** — a thin command-line companion (the installed binary is `colophon`).

## Filesystem

colophon is generic over *where* documents live. It depends on no concrete backend; instead it asks integrators to implement the small async [`colophon::Storage`](colophon/src/fs.rs) trait, which mirrors the slice of `std::fs` the scan/traverse/mutate engine needs. Implement it over `std::fs`, `tokio::fs`, or a browser filesystem (OPFS/IndexedDB) — the workspace never learns which.

## Status

Early extraction from [diaryx](https://github.com/diaryx-org/diaryx). The pure layers — embedded-metadata parsing, document splitting, relation extraction — are real and tested. The filesystem-driven scan/traversal/mutation engine ports next; its seams (`Workspace`, `IdentityPolicy`, `IndexStore`, `Storage`) are staked out so nothing app-specific leaks into the eventual public API.

## License

Not licensed for now.