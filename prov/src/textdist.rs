//! Small string edit-distance helpers, shared by the config linter
//! ([`crate::config::diagnose`]) and the vocabulary term-consistency pass
//! ([`crate::validate`]). Both need the same "is this a likely typo of a known
//! spelling?" judgment — a misspelled config key, a drifted tag — so the metric
//! lives in one place rather than being copied per call site.

/// The candidate in `candidates` that most resembles `key`, when one is within a
/// small edit distance (a likely typo) — else `None`. Distance is measured
/// case-sensitively so a case-only slip surfaces its canonical spelling. The
/// threshold (2) is deliberately tight: recognized spellings are distinctive
/// enough that structural fields (`title`, `part_of`, `id`) and ordinary user
/// values fall outside it, so they are never mistaken for typos.
pub(crate) fn nearest(key: &str, candidates: &[&str]) -> Option<String> {
    candidates
        .iter()
        .map(|cand| (levenshtein(key, cand), *cand))
        .filter(|(d, _)| (1..=2).contains(d))
        .min_by_key(|(d, _)| *d)
        .map(|(_, cand)| cand.to_string())
}

/// The candidate string (owned) in `candidates` nearest to `key` within the
/// typo threshold — the `String`-slice form of [`nearest`], for callers whose
/// candidate set is built at runtime (vocabulary term names) rather than a
/// static `&[&str]` (config axis names).
pub(crate) fn nearest_owned(key: &str, candidates: &[String]) -> Option<String> {
    candidates
        .iter()
        .map(|cand| (levenshtein(key, cand), cand))
        .filter(|(d, _)| (1..=2).contains(d))
        .min_by_key(|(d, _)| *d)
        .map(|(_, cand)| cand.clone())
}

/// Levenshtein edit distance — the classic two-row dynamic program.
pub(crate) fn levenshtein(a: &str, b: &str) -> usize {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_finds_a_close_typo_and_ignores_distant_words() {
        assert_eq!(
            nearest("recyle_bin", &["recycle_bin", "fixity"]),
            Some("recycle_bin".to_string())
        );
        // A word within threshold but not identical is a hit; a far word is not.
        assert_eq!(nearest("author", &["recycle_bin", "fixity"]), None);
        // An exact match is distance 0 — deliberately not a "typo".
        assert_eq!(nearest("fixity", &["fixity"]), None);
    }

    #[test]
    fn nearest_owned_matches_the_slice_form() {
        let cands = vec!["public".to_string(), "friends".to_string()];
        assert_eq!(
            nearest_owned("freinds", &cands),
            Some("friends".to_string())
        );
        assert_eq!(nearest_owned("colleagues", &cands), None);
    }
}
