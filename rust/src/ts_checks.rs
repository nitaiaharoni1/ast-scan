//! TypeScript-specific checks (import boundaries, shared types).
//! Per-file AST work lives in `ts_scanner`.

use crate::types::BoundaryRule;

/// Parse `--boundary source:forbidden1,forbidden2` (same rules as the historical TS CLI).
pub(crate) fn parse_boundary_flag(value: &str) -> Option<BoundaryRule> {
    let colon_idx = value.find(':')?;
    if colon_idx < 1 {
        return None;
    }
    let source = value[..colon_idx].to_string();
    let forbidden: Vec<String> = value[colon_idx + 1..]
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if forbidden.is_empty() {
        return None;
    }
    Some(BoundaryRule { source, forbidden })
}
