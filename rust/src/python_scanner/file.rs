//! Per-file Python scanning (`analyze_py_file`).

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use rustpython_parser::{
    ast::Ranged,
    ast::{self, Constant, Expr, Mod, Stmt},
    parse, Mode,
};

use crate::audits::collect_todo_comments;
use crate::types::{ImportEdge, PyClassInfo, PyFileData, PyFuncInfo, RouteInfo};

use super::visitors::{
    collect_mutable_defaults, collect_py_security, collect_silent_excepts, collect_star_import,
    compute_cognitive_complexity, compute_complexity, compute_max_nesting, count_python_params,
    decorator_repr, extract_route, line_at, line_at_end, process_import, python_body_shape_hash,
    python_func_exact_hash,
};
use super::{display_rel, file_to_module};

/// Append string literals from `__all__ = [...]` / tuple / set initializers.
fn collect_all_export_strings(expr: &Expr, exports: &mut Vec<String>) {
    match expr {
        Expr::List(l) => {
            for elt in &l.elts {
                if let Expr::Constant(c) = elt {
                    if let Constant::Str(s) = &c.value {
                        exports.push(s.clone());
                    }
                }
            }
        }
        Expr::Tuple(t) => {
            for elt in &t.elts {
                if let Expr::Constant(c) = elt {
                    if let Constant::Str(s) = &c.value {
                        exports.push(s.clone());
                    }
                }
            }
        }
        Expr::Set(s) => {
            for elt in &s.elts {
                if let Expr::Constant(c) = elt {
                    if let Constant::Str(st) = &c.value {
                        exports.push(st.clone());
                    }
                }
            }
        }
        _ => {}
    }
}

fn is_python_test_path(rel: &str) -> bool {
    let lower = rel.replace('\\', "/").to_ascii_lowercase();
    if lower.contains("/tests/") || lower.starts_with("tests/") {
        return true;
    }
    Path::new(rel)
        .file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|n| {
            n.starts_with("test_")
                || n.ends_with("_test.py")
                || n.ends_with("_tests.py")
        })
}

struct FileAnalyzer<'a> {
    filepath: &'a str,
    module: &'a str,
    pkg: &'a str,
    source: &'a str,
    is_test_file: bool,
    functions: Vec<PyFuncInfo>,
    classes: Vec<PyClassInfo>,
    imports: Vec<ImportEdge>,
    exports: Vec<String>,
    top_level_names: Vec<String>,
    routes: Vec<RouteInfo>,
    mutable_defaults: Vec<crate::types::MutableDefaultInfo>,
    star_imported_modules: Vec<String>,
    class_stack: Vec<String>,
}

impl<'a> FileAnalyzer<'a> {
    fn new(filepath: &'a str, module: &'a str, pkg: &'a str, source: &'a str, is_test_file: bool) -> Self {
        Self {
            filepath,
            module,
            pkg,
            source,
            is_test_file,
            functions: Vec::new(),
            classes: Vec::new(),
            imports: Vec::new(),
            exports: Vec::new(),
            top_level_names: Vec::new(),
            routes: Vec::new(),
            mutable_defaults: Vec::new(),
            star_imported_modules: Vec::new(),
            class_stack: Vec::new(),
        }
    }

    fn process_function(&mut self, node: &ast::StmtFunctionDef, is_method: bool) -> PyFuncInfo {
        let line = line_at(self.source, node.range().start());
        let end_line = line_at_end(self.source, node.range().end());
        let qualname = if self.class_stack.is_empty() {
            node.name.to_string()
        } else {
            format!("{}.{}", self.class_stack.join("."), node.name)
        };
        let complexity = compute_complexity(&node.body);
        let cognitive_complexity = compute_cognitive_complexity(&node.body);
        let nesting = compute_max_nesting(&node.body);
        let param_count = count_python_params(&node.args, is_method);
        let clone_hash = python_body_shape_hash(&node.body);
        let start = usize::from(node.range().start());
        let end = usize::from(node.range().end());
        let exact_clone_hash = python_func_exact_hash(self.source, start, end);
        let decorators: Vec<_> = node.decorator_list.iter().map(decorator_repr).collect();
        let info = PyFuncInfo {
            name: node.name.to_string(),
            qualname: qualname.clone(),
            file: self.filepath.to_string(),
            line,
            end_line,
            line_count: end_line.saturating_sub(line) + 1,
            complexity,
            cognitive_complexity,
            nesting,
            param_count,
            clone_hash,
            exact_clone_hash,
            decorators: decorators.clone(),
            is_method,
            is_test: self.is_test_file,
        };
        let mut mds = collect_mutable_defaults(
            &node.args,
            &qualname,
            self.filepath,
            self.source,
        );
        self.mutable_defaults.append(&mut mds);
        if let Some(r) = extract_route(
            node.name.as_ref(),
            &qualname,
            self.filepath,
            line,
            &node.decorator_list,
            &node.body,
        ) {
            self.routes.push(r);
        }
        self.functions.push(info.clone());
        info
    }

    fn process_async_function(
        &mut self,
        node: &ast::StmtAsyncFunctionDef,
        is_method: bool,
    ) -> PyFuncInfo {
        let line = line_at(self.source, node.range().start());
        let end_line = line_at_end(self.source, node.range().end());
        let qualname = if self.class_stack.is_empty() {
            node.name.to_string()
        } else {
            format!("{}.{}", self.class_stack.join("."), node.name)
        };
        let complexity = compute_complexity(&node.body);
        let cognitive_complexity = compute_cognitive_complexity(&node.body);
        let nesting = compute_max_nesting(&node.body);
        let param_count = count_python_params(&node.args, is_method);
        let clone_hash = python_body_shape_hash(&node.body);
        let start = usize::from(node.range().start());
        let end = usize::from(node.range().end());
        let exact_clone_hash = python_func_exact_hash(self.source, start, end);
        let decorators: Vec<_> = node.decorator_list.iter().map(decorator_repr).collect();
        let info = PyFuncInfo {
            name: node.name.to_string(),
            qualname: qualname.clone(),
            file: self.filepath.to_string(),
            line,
            end_line,
            line_count: end_line.saturating_sub(line) + 1,
            complexity,
            cognitive_complexity,
            nesting,
            param_count,
            clone_hash,
            exact_clone_hash,
            decorators: decorators.clone(),
            is_method,
            is_test: self.is_test_file,
        };
        let mut mds = collect_mutable_defaults(
            &node.args,
            &qualname,
            self.filepath,
            self.source,
        );
        self.mutable_defaults.append(&mut mds);
        if let Some(r) = extract_route(
            node.name.as_ref(),
            &qualname,
            self.filepath,
            line,
            &node.decorator_list,
            &node.body,
        ) {
            self.routes.push(r);
        }
        self.functions.push(info.clone());
        info
    }

    fn process_class(&mut self, node: &ast::StmtClassDef) {
        let line = line_at(self.source, node.range().start());
        let end_line = line_at_end(self.source, node.range().end());
        let mut cls = PyClassInfo {
            name: node.name.to_string(),
            file: self.filepath.to_string(),
            line,
            end_line,
            line_count: end_line.saturating_sub(line) + 1,
            methods: Vec::new(),
            decorators: node.decorator_list.iter().map(decorator_repr).collect(),
        };
        self.class_stack.push(node.name.to_string());
        for item in &node.body {
            match item {
                Stmt::FunctionDef(f) => {
                    let fi = self.process_function(f, true);
                    cls.methods.push(fi);
                }
                Stmt::AsyncFunctionDef(f) => {
                    let fi = self.process_async_function(f, true);
                    cls.methods.push(fi);
                }
                _ => {}
            }
        }
        self.class_stack.pop();
        self.classes.push(cls);
    }

    fn visit_module(&mut self, body: &[Stmt]) {
        for item in body {
            match item {
                Stmt::FunctionDef(f) => {
                    self.process_function(f, false);
                    self.top_level_names.push(f.name.to_string());
                }
                Stmt::AsyncFunctionDef(f) => {
                    self.process_async_function(f, false);
                    self.top_level_names.push(f.name.to_string());
                }
                Stmt::ClassDef(c) => {
                    self.process_class(c);
                    self.top_level_names.push(c.name.to_string());
                }
                Stmt::Assign(a) => {
                    for t in &a.targets {
                        if let Expr::Name(n) = t {
                            self.top_level_names.push(n.id.to_string());
                            if n.id.as_str() == "__all__" {
                                collect_all_export_strings(a.value.as_ref(), &mut self.exports);
                            }
                        }
                    }
                }
                Stmt::AnnAssign(a) => {
                    if let Expr::Name(n) = a.target.as_ref() {
                        self.top_level_names.push(n.id.to_string());
                    }
                }
                Stmt::Import(_) | Stmt::ImportFrom(_) => {
                    process_import(item, self.module, self.pkg, &mut self.imports);
                    if let Some(star_mod) = collect_star_import(item, self.pkg) {
                        self.star_imported_modules.push(star_mod);
                    }
                }
                _ => {}
            }
        }
    }
}

pub(super) enum PyFileScanItem {
    /// Boxed so the enum stays small on worker-thread stacks (clippy large_enum_variant).
    Data(Box<PyFileData>),
    ParseError {
        rel: String,
        message: String,
    },
}

pub(super) fn analyze_py_file(fpath: &Path, scan_root: &Path, pkg: &str) -> Option<PyFileScanItem> {
    let module = file_to_module(fpath, scan_root, pkg);
    let source = fs::read_to_string(fpath)
        .unwrap_or_default()
        .replace('\r', "");
    let rel = display_rel(fpath, scan_root);
    let line_count = source.matches('\n').count() + 1;

    let body = match parse(&source, Mode::Module, &fpath.display().to_string()) {
        Ok(Mod::Module(m)) => m.body,
        Ok(_) => return None,
        Err(e) => {
            return Some(PyFileScanItem::ParseError {
                rel,
                message: e.to_string(),
            });
        }
    };

    let abs = fpath.display().to_string();
    let is_test_file = is_python_test_path(&rel);
    let mut an = FileAnalyzer::new(&abs, &module, pkg, &source, is_test_file);
    an.visit_module(&body);

    let mut todo_freq = HashMap::new();
    let mut todo_samples = HashMap::new();
    collect_todo_comments(&source, &rel, &mut todo_freq, &mut todo_samples);

    let mut silent_excepts = Vec::new();
    collect_silent_excepts(&body, &rel, &source, &mut silent_excepts);

    let mut security_findings = Vec::new();
    collect_py_security(&body, &rel, &source, &mut security_findings);

    Some(PyFileScanItem::Data(Box::new(PyFileData {
        module: module.clone(),
        rel_path: rel,
        line_count,
        functions: an.functions,
        classes: an.classes,
        imports: an.imports,
        top_level_names: an.top_level_names,
        routes: an.routes,
        mutable_defaults: an.mutable_defaults,
        star_imported_modules: an.star_imported_modules,
        silent_excepts,
        todo_freq,
        todo_samples,
        security_findings,
        is_test_file,
    })))
}
