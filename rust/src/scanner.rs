//! Unified scanner interface for Python, TypeScript, and Rust analysis.

use std::collections::HashSet;
use std::path::Path;

use serde_json::Value;

use crate::report;
use crate::ts_scanner;

/// Active scan language (fixed order when multiple: Python, TypeScript, Rust).
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum ScanMode {
    Python,
    TypeScript,
    Rust,
}

impl ScanMode {
    pub(crate) fn json_key(self) -> &'static str {
        match self {
            ScanMode::Python => "python",
            ScanMode::TypeScript => "typescript",
            ScanMode::Rust => "rust",
        }
    }

    pub(crate) fn threshold_label(self) -> &'static str {
        match self {
            ScanMode::Python => "python",
            ScanMode::TypeScript => "typescript",
            ScanMode::Rust => "rust",
        }
    }
}

/// One language scanner: produce JSON and optional text report.
pub(crate) trait Scanner: Send + Sync {
    fn mode(&self) -> ScanMode;

    fn analyze(&self, root: &Path, exclude: &[String]) -> anyhow::Result<Value>;

    /// Print text report. Returns `true` when TypeScript import-boundary violations were printed
    /// (caller should treat as failure), matching historical CLI behavior.
    fn print_report(&self, data: &Value, title: &str, top: usize, skip: &HashSet<String>) -> bool;
}

/// Python package scan (`--pkg` / default).
pub(crate) struct PythonScanner {
    pub(crate) pkg: String,
}

impl Scanner for PythonScanner {
    fn mode(&self) -> ScanMode {
        ScanMode::Python
    }

    fn analyze(&self, root: &Path, exclude: &[String]) -> anyhow::Result<Value> {
        crate::python_scanner::analyze_python(root, &self.pkg, exclude)
    }

    fn print_report(&self, data: &Value, title: &str, top: usize, skip: &HashSet<String>) -> bool {
        report::print_python_report(data, title, top, skip);
        false
    }
}

/// TypeScript / JavaScript scan.
pub(crate) struct TypeScriptScanner {
    pub(crate) alias: String,
    pub(crate) cfg: ts_scanner::AnalysisConfig,
}

impl Scanner for TypeScriptScanner {
    fn mode(&self) -> ScanMode {
        ScanMode::TypeScript
    }

    fn analyze(&self, root: &Path, exclude: &[String]) -> anyhow::Result<Value> {
        let mut cfg = self.cfg.clone();
        cfg.exclude = exclude.to_vec();
        crate::ts_scanner::analyze_typescript(root, &self.alias, &cfg)
    }

    fn print_report(&self, data: &Value, title: &str, top: usize, skip: &HashSet<String>) -> bool {
        report::print_ts_report(data, title, top, skip)
    }
}

/// Rust `.rs` scan.
pub(crate) struct RustScanner;

impl Scanner for RustScanner {
    fn mode(&self) -> ScanMode {
        ScanMode::Rust
    }

    fn analyze(&self, root: &Path, exclude: &[String]) -> anyhow::Result<Value> {
        crate::rust_scanner::analyze_rust(root, exclude)
    }

    fn print_report(&self, data: &Value, title: &str, top: usize, skip: &HashSet<String>) -> bool {
        report::print_rust_report(data, title, top, skip);
        false
    }
}
