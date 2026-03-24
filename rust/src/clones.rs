//! Normalized AST shape hashing for duplicate (Type-2) clone detection.

use std::hash::{DefaultHasher, Hash, Hasher};

/// Stable hash of a structural fingerprint string (identifiers / literals normalized away upstream).
pub(crate) fn hash_shape(payload: &str) -> u64 {
    let mut h = DefaultHasher::new();
    payload.hash(&mut h);
    h.finish()
}
