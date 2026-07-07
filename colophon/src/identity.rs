//! Identity — a strictly-additive layer over a path-only workspace.
//!
//! Everything below is optional. The graph and (eventually) mutation operate on
//! paths and never require an ID. This module only decides **when** a document
//! earns a stable ID (the trigger set) and **what** that ID looks like (the
//! mint). *Where* IDs are stored is [`crate::index`].
//!
//! The default is [`NoIdentity`] — identity off, no ID ever written. The
//! recommended lazy policy registers an ID only when something durably refers to
//! a document (a link-by-id or a publish), keeping the authoritative set as small
//! as possible.

use std::path::Path;

/// A stable, opaque document identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Id(pub String);

impl Id {
    /// The id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Which events cause a document to be assigned (registered) an ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Registration {
    /// Register every document at creation time (eager).
    pub on_create: bool,
    /// Register when a document is first referenced by ID (e.g. a wikilink).
    pub on_link: bool,
    /// Register when a document is published.
    pub on_publish: bool,
}

impl Registration {
    /// Never register — identity is effectively off.
    pub const OFF: Self = Self { on_create: false, on_link: false, on_publish: false };
    /// Register only on a durable reference (link-by-id or publish). Recommended.
    pub const LAZY: Self = Self { on_create: false, on_link: true, on_publish: true };
    /// Register every document the moment it is created.
    pub const EAGER: Self = Self { on_create: true, on_link: true, on_publish: true };

    /// Whether any trigger is active.
    pub fn is_active(&self) -> bool {
        self.on_create || self.on_link || self.on_publish
    }
}

/// A policy deciding when to register documents and how their IDs are minted.
pub trait IdentityPolicy {
    /// The registration trigger set for this policy.
    fn registration(&self) -> Registration;

    /// Mint a fresh ID for the document at `path`. Only called when a trigger
    /// fires, so a disabled policy need never produce a meaningful value.
    fn mint(&mut self, path: &Path) -> Id;
}

/// Identity disabled — the default. Paths only; no ID is ever minted or written.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoIdentity;

impl IdentityPolicy for NoIdentity {
    fn registration(&self) -> Registration {
        Registration::OFF
    }

    fn mint(&mut self, _path: &Path) -> Id {
        // Unreachable in practice: `OFF` fires no triggers.
        Id(String::new())
    }
}

/// A minting identity policy with a configurable trigger set.
///
/// The ID scheme here is a placeholder (sequential base-36) so the seams are
/// exercised; a real deployment swaps [`Minter::mint`] for ULID, or — in
/// diaryx — an ARK blade.
#[derive(Debug, Clone)]
pub struct Minter {
    registration: Registration,
    next: u64,
}

impl Minter {
    /// Register only on a durable reference (the recommended default).
    pub fn lazy() -> Self {
        Self { registration: Registration::LAZY, next: 0 }
    }

    /// Register every document at creation.
    pub fn eager() -> Self {
        Self { registration: Registration::EAGER, next: 0 }
    }

    /// Register on a custom trigger set.
    pub fn with(registration: Registration) -> Self {
        Self { registration, next: 0 }
    }
}

impl IdentityPolicy for Minter {
    fn registration(&self) -> Registration {
        self.registration
    }

    fn mint(&mut self, _path: &Path) -> Id {
        let n = self.next;
        self.next += 1;
        Id(to_base36(n))
    }
}

/// Render `n` in base-36 (0-9a-z), least-significant digit last. `0` → `"0"`.
fn to_base36(mut n: u64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".to_string();
    }
    let mut buf = Vec::new();
    while n > 0 {
        buf.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    buf.reverse();
    String::from_utf8(buf).expect("base-36 digits are ASCII")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_identity_is_off() {
        assert!(!NoIdentity.registration().is_active());
    }

    #[test]
    fn lazy_registers_on_link_and_publish_only() {
        let p = Minter::lazy();
        let r = p.registration();
        assert!(!r.on_create && r.on_link && r.on_publish);
    }

    #[test]
    fn eager_registers_on_create() {
        assert!(Minter::eager().registration().on_create);
    }

    #[test]
    fn mints_distinct_ids() {
        let mut p = Minter::eager();
        let a = p.mint(Path::new("a.md"));
        let b = p.mint(Path::new("b.md"));
        assert_ne!(a, b);
        assert_eq!(a, Id("0".into()));
        assert_eq!(b, Id("1".into()));
    }

    #[test]
    fn base36_wraps_past_ten() {
        assert_eq!(to_base36(35), "z");
        assert_eq!(to_base36(36), "10");
    }
}
