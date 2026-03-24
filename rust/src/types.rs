use std::collections::HashMap;

use serde::Serialize;

// ── Python-specific types ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PyFuncInfo {
    pub name: String,
    pub qualname: String,
    pub file: String,
    pub line: usize,
    pub end_line: usize,
    pub line_count: usize,
    pub complexity: usize,
    pub nesting: usize,
    pub decorators: Vec<String>,
    pub is_method: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PyClassInfo {
    pub name: String,
    pub file: String,
    pub line: usize,
    pub end_line: usize,
    pub line_count: usize,
    pub methods: Vec<PyFuncInfo>,
    pub decorators: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RouteInfo {
    pub method: String,
    pub path: String,
    pub handler: String,
    pub file: String,
    pub line: usize,
    pub dependencies: Vec<String>,
}

// ── TypeScript-specific types ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TsFuncInfo {
    pub name: String,
    pub file: String,
    pub line: usize,
    pub end_line: usize,
    pub line_count: usize,
    pub complexity: usize,
    pub nesting: usize,
    pub exported: bool,
    pub is_component: bool,
    pub props: Vec<String>,
    pub hooks: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TsClassInfo {
    pub name: String,
    pub file: String,
    pub line: usize,
    pub line_count: usize,
    pub methods: usize,
    pub properties: usize,
    pub exported: bool,
    pub has_heritage: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TsImportInfo {
    pub source: String,
    pub specifiers: Vec<String>,
    pub is_internal: bool,
    pub resolved_path: String,
    /// Source line of the import declaration (for boundary reporting).
    #[serde(skip)]
    pub line: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct TsFileData {
    pub rel_path: String,
    pub abs_path: String,
    pub line_count: usize,
    pub functions: Vec<TsFuncInfo>,
    pub classes: Vec<TsClassInfo>,
    pub imports: Vec<TsImportInfo>,
    pub exports: Vec<String>,
    pub source: String,
    pub any_count: usize,
    pub console_debugger: Vec<ConsoleDebuggerInfo>,
    pub silent_catches: Vec<SilentCatchInfo>,
    pub mobx_observer_issues: Vec<MobxObserverInfo>,
    pub orm_case_issues: Vec<OrmCaseFinding>,
}

// ── Shared types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ImportEdge {
    pub source_module: String,
    pub target_module: String,
    pub names: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SilentCatchInfo {
    pub file: String,
    pub line: usize,
    pub kind: String,
}

/// Per-file Python analysis result (parallel phase); aggregated sequentially afterward.
#[derive(Debug, Clone)]
pub(crate) struct PyFileData {
    pub module: String,
    pub rel_path: String,
    pub line_count: usize,
    pub functions: Vec<PyFuncInfo>,
    pub classes: Vec<PyClassInfo>,
    pub imports: Vec<ImportEdge>,
    pub top_level_names: Vec<String>,
    pub routes: Vec<RouteInfo>,
    pub silent_excepts: Vec<SilentCatchInfo>,
    pub todo_freq: HashMap<String, usize>,
    pub todo_samples: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MobxObserverInfo {
    pub file: String,
    pub line: usize,
    pub component: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct OrmCaseFinding {
    pub file: String,
    pub line: usize,
    pub method: String,
    pub snippet: String,
    pub identifier: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct BoundaryRule {
    pub source: String,
    pub forbidden: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct BoundaryViolation {
    pub file: String,
    pub line: usize,
    pub import_source: String,
    pub rule: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ConsoleDebuggerInfo {
    pub file: String,
    pub line: usize,
    pub kind: String,
}

// ── Rust-specific types ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RsFuncInfo {
    pub name: String,
    pub qualname: String,
    pub file: String,
    pub line: usize,
    pub end_line: usize,
    pub line_count: usize,
    pub complexity: usize,
    pub nesting: usize,
    pub is_method: bool,
    pub is_unsafe: bool,
    pub is_async: bool,
    pub visibility: String,
    /// `Some("MyType")` for methods inside `impl MyType`.
    pub parent_type: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RsStructInfo {
    pub name: String,
    pub kind: String, // "struct" | "enum"
    pub file: String,
    pub line: usize,
    pub line_count: usize,
    pub fields_count: usize,
    pub methods_count: usize,
    pub derives: Vec<String>,
    pub has_generics: bool,
    pub visibility: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RsTraitInfo {
    pub name: String,
    pub file: String,
    pub line: usize,
    pub visibility: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RsImportInfo {
    pub source: String,
    pub specifiers: Vec<String>,
    pub is_internal: bool,
    pub resolved_path: String,
    #[serde(skip)]
    #[allow(dead_code)]
    pub line: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct RsFileData {
    pub rel_path: String,
    #[allow(dead_code)]
    pub abs_path: String,
    pub line_count: usize,
    pub source: String,
    pub functions: Vec<RsFuncInfo>,
    pub(crate) structs: Vec<RsStructInfo>,
    pub traits: Vec<RsTraitInfo>,
    pub imports: Vec<RsImportInfo>,
    pub exports: Vec<String>,
    pub unsafe_blocks: usize,
    pub unwrap_expect_count: usize,
    /// Lint names from `#[allow(...)]` on this file's items (aggregated).
    pub allow_lint_hits: HashMap<String, usize>,
    /// Derive macro names used in this file.
    pub derive_hits: HashMap<String, usize>,
}
