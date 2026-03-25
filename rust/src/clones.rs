//! Normalized AST shape hashing for duplicate (Type-2) clone detection.

use std::hash::{DefaultHasher, Hash, Hasher};

/// Stable hash of a structural fingerprint string (identifiers / literals normalized away upstream).
pub(crate) fn hash_shape(payload: &str) -> u64 {
    let mut h = DefaultHasher::new();
    payload.hash(&mut h);
    h.finish()
}

/// Hash a raw source slice normalised for whitespace only (Type-1 clone detection).
/// Each line is trimmed; blank lines removed; joined with `\n`.
pub(crate) fn hash_exact(payload: &str) -> u64 {
    let normalized: String = payload
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let mut h = DefaultHasher::new();
    normalized.hash(&mut h);
    h.finish()
}
