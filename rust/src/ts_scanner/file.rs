//! Per-file TypeScript/JavaScript analysis using `oxc_parser`.

use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use oxc_allocator::Allocator;
use oxc_ast::ast::*;
use oxc_parser::Parser;
use oxc_span::SourceType;

use crate::types::{
    BoundaryRule, BoundaryViolation, MobxObserverInfo, TsClassInfo, TsFileData,
    TsFuncInfo, TsImportInfo,
};

use super::visitors::{
    body_contains_jsx, cognitive_expression, cognitive_function_body, collect_console_debugger,
    collect_hooks_in_body, collect_hooks_in_expr, collect_orm_case_findings, collect_silent_catches,
    collect_ts_security, complexity_expression, complexity_function_body, count_any_in_program,
    count_ts_formal_params, expr_contains_jsx, nesting_expression, nesting_function_body,
    ts_expression_shape_hash, ts_function_body_shape_hash,
};

fn exact_hash_from_span(source: &str, start: u32, end: u32) -> u64 {
    let s = start as usize;
    let e = (end as usize).min(source.len());
    crate::clones::hash_exact(source.get(s..e).unwrap_or(""))
}

pub(crate) fn display_rel(abs: &Path, scan_root: &Path) -> String {
    if let Some(parent) = scan_root.parent() {
        if let Ok(p) = abs.strip_prefix(parent) {
            return p.display().to_string();
        }
    }
    abs.display().to_string()
}

fn line_at(source: &str, offset: u32) -> usize {
    let o = offset as usize;
    if o > source.len() {
        return 1;
    }
    source[..o].bytes().filter(|&b| b == b'\n').count() + 1
}

pub(crate) fn resolve_import(
    import_path: &str,
    from_file: &Path,
    scan_root: &Path,
    alias_prefix: &str,
) -> (bool, String) {
    if let Some(mapped) = import_path.strip_prefix(alias_prefix) {
        return (true, mapped.to_string());
    }
    if import_path.starts_with('.') {
        let from_rel = from_file.strip_prefix(scan_root).unwrap_or(from_file);
        let dir = from_rel.parent().unwrap_or_else(|| Path::new(""));
        let joined = dir.join(import_path);
        let with_slash = normalize_slash(joined.to_string_lossy().as_ref());
        return (true, collapse_posix_rel_path(&with_slash));
    }
    (false, import_path.to_string())
}

fn normalize_slash(p: &str) -> String {
    p.replace('\\', "/")
}

/// Collapse `.` / `..` in a relative module id (POSIX-style), mirroring `path.posix.normalize`.
fn collapse_posix_rel_path(path_str: &str) -> String {
    let mut out = PathBuf::new();
    for c in Path::new(path_str).components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(p) => out.push(p),
            Component::Prefix(_) | Component::RootDir => {}
        }
    }
    normalize_slash(&out.to_string_lossy())
}

pub(crate) fn normalize_module_path(p: &str) -> String {
    let mut cleaned = p.replace('\\', "/");
    for ext in [".tsx", ".ts", ".jsx", ".js"] {
        if cleaned.ends_with(ext) {
            cleaned.truncate(cleaned.len() - ext.len());
            break;
        }
    }
    if cleaned.ends_with("/index") {
        cleaned.truncate(cleaned.len() - "/index".len());
    }
    cleaned
}

/// Skip heavy / non-source trees (aligns with `main::should_skip_dir` for Python walks).
fn should_skip_ts_dir(name: &str) -> bool {
    matches!(
        name,
        "node_modules"
            | ".git"
            | "dist"
            | "build"
            | "target"
            | ".venv"
            | "venv"
            | ".next"
            | ".turbo"
    )
}

fn matches_exclude(filepath: &Path, scan_root: &Path, patterns: &[String]) -> bool {
    let Ok(rel) = filepath.strip_prefix(scan_root) else {
        return false;
    };
    let rel = rel.display().to_string();
    patterns
        .iter()
        .any(|pat| rel.contains(pat) || rel.starts_with(pat))
}

fn is_ts_test_path(rel: &str) -> bool {
    let r = rel.replace('\\', "/").to_ascii_lowercase();
    r.contains("/__tests__/")
        || r.contains(".test.")
        || r.contains(".spec.")
        || r.contains(".cy.")
        || r.contains(".jest.")
        || r.ends_with("_test.ts")
        || r.ends_with("_test.tsx")
        || r.ends_with("_test.js")
        || r.ends_with("_test.jsx")
}

pub(crate) fn collect_ts_files(scan_root: &Path, exclude: &[String]) -> Vec<PathBuf> {
    let mut result = Vec::new();
    let mut stack = vec![scan_root.to_path_buf()];
    let exts = [".ts", ".tsx", ".js", ".jsx"];
    while let Some(dp) = stack.pop() {
        let read_dir = match fs::read_dir(&dp) {
            Ok(d) => d,
            Err(_) => continue,
        };
        for ent in read_dir.flatten() {
            let full = ent.path();
            let name = ent.file_name();
            let n = name.to_string_lossy();
            if ent.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                if should_skip_ts_dir(n.as_ref()) {
                    continue;
                }
                if !exclude.is_empty() && matches_exclude(&full, scan_root, exclude) {
                    continue;
                }
                stack.push(full);
            } else if exts.iter().any(|e| n.ends_with(e))
                && (exclude.is_empty() || !matches_exclude(&full, scan_root, exclude))
            {
                result.push(full);
            }
        }
    }
    result.sort();
    result
}

fn module_export_name_str(n: &ModuleExportName) -> String {
    match n {
        ModuleExportName::IdentifierName(i) => i.name.as_str().to_string(),
        ModuleExportName::IdentifierReference(i) => i.name.as_str().to_string(),
        ModuleExportName::StringLiteral(s) => s.value.as_str().to_string(),
    }
}

fn property_key_method_name(key: &PropertyKey) -> Option<String> {
    match key {
        PropertyKey::StaticIdentifier(id) => Some(id.name.as_str().to_string()),
        PropertyKey::PrivateIdentifier(p) => Some(format!("#{}", p.name.as_str())),
        _ => None,
    }
}

fn extract_props_from_params<'a>(params: &FormalParameters<'a>) -> Vec<String> {
    let Some(first) = params.items.first() else {
        return vec![];
    };
    match &first.pattern {
        BindingPattern::ObjectPattern(obj) => {
            let mut props = Vec::new();
            for prop in &obj.properties {
                if prop.shorthand {
                    if let PropertyKey::StaticIdentifier(id) = &prop.key {
                        props.push(id.name.as_str().to_string());
                    }
                } else if let BindingPattern::BindingIdentifier(id) = &prop.value {
                    props.push(id.name.as_str().to_string());
                }
            }
            props
        }
        BindingPattern::BindingIdentifier(_) => {
            if let Some(ann) = &first.type_annotation {
                if let TSType::TSTypeReference(tr) = &ann.type_annotation {
                    if let TSTypeName::IdentifierReference(ir) = &tr.type_name {
                        return vec![format!("[type: {}]", ir.name.as_str())];
                    }
                }
            }
            vec![]
        }
        _ => vec![],
    }
}

fn extract_props_function<'a>(func: &Function<'a>) -> Vec<String> {
    extract_props_from_params(&func.params)
}

fn extract_props_arrow<'a>(arr: &ArrowFunctionExpression<'a>) -> Vec<String> {
    extract_props_from_params(&arr.params)
}

fn body_metrics_jsx_hooks_function<'a>(
    body: &FunctionBody<'a>,
) -> (usize, usize, bool, Vec<String>) {
    (
        complexity_function_body(body),
        nesting_function_body(body),
        body_contains_jsx(body),
        collect_hooks_in_body(body),
    )
}

fn body_metrics_jsx_hooks_arrow<'a>(
    arr: &ArrowFunctionExpression<'a>,
) -> (usize, usize, bool, Vec<String>) {
    if arr.expression {
        if let Some(expr) = arr.get_expression() {
            (
                complexity_expression(expr),
                nesting_expression(expr),
                expr_contains_jsx(expr),
                collect_hooks_in_expr(expr),
            )
        } else {
            (1, 0, false, vec![])
        }
    } else {
        (
            complexity_function_body(&arr.body),
            nesting_function_body(&arr.body),
            body_contains_jsx(&arr.body),
            collect_hooks_in_body(&arr.body),
        )
    }
}

fn is_component_name(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_ascii_uppercase()) && !name.ends_with("Provider")
}

fn ingest_import_declaration(
    im: &ImportDeclaration<'_>,
    abs: &Path,
    scan_root: &Path,
    alias_prefix: &str,
    file_src: &str,
    imports: &mut Vec<TsImportInfo>,
) {
    let imp_path = im.source.value.as_str();
    let (is_internal, resolved) = resolve_import(imp_path, abs, scan_root, alias_prefix);
    let mut specifiers = Vec::new();
    if let Some(specs) = &im.specifiers {
        for s in specs {
            match s {
                ImportDeclarationSpecifier::ImportSpecifier(sp) => {
                    specifiers.push(module_export_name_str(&sp.imported));
                }
                ImportDeclarationSpecifier::ImportDefaultSpecifier(d) => {
                    specifiers.push(d.local.name.as_str().to_string());
                }
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(n) => {
                    specifiers.push(format!("* as {}", n.local.name.as_str()));
                }
            }
        }
    }
    imports.push(TsImportInfo {
        source: imp_path.to_string(),
        specifiers,
        is_internal,
        resolved_path: resolved,
        line: line_at(file_src, im.span.start),
    });
}

/// Per-file scan context for function metrics (avoids long `process_*` argument lists).
struct TsFuncScanState<'a> {
    functions: &'a mut Vec<TsFuncInfo>,
    file: &'a str,
    source: &'a str,
    is_tsx: bool,
    is_test_file: bool,
}

fn process_function<'a>(
    st: &mut TsFuncScanState<'_>,
    exports: &mut Vec<String>,
    name: String,
    func: &Function<'a>,
    exported: bool,
) {
    let Some(body) = func.body.as_ref() else {
        return;
    };
    let start = line_at(st.source, func.span.start);
    let end = line_at(st.source, func.span.end);
    let (cc, nest, jsx, hooks) = body_metrics_jsx_hooks_function(body);
    let cognitive_complexity = cognitive_function_body(body);
    let param_count = count_ts_formal_params(&func.params);
    let clone_hash = ts_function_body_shape_hash(body);
    let exact_clone_hash = exact_hash_from_span(st.source, func.span.start, func.span.end);
    let is_comp = st.is_tsx && is_component_name(&name) && jsx;
    let props = if is_comp {
        extract_props_function(func)
    } else {
        vec![]
    };
    let hooks = if is_comp { hooks } else { vec![] };
    st.functions.push(TsFuncInfo {
        name: name.clone(),
        file: st.file.to_string(),
        line: start,
        end_line: end,
        line_count: end.saturating_sub(start) + 1,
        complexity: cc,
        cognitive_complexity,
        nesting: nest,
        param_count,
        clone_hash,
        exact_clone_hash,
        exported,
        is_component: is_comp,
        props,
        hooks,
        is_test: st.is_test_file,
    });
    if exported {
        exports.push(name);
    }
}

fn process_arrow_var<'a>(
    st: &mut TsFuncScanState<'_>,
    exports: &mut Vec<String>,
    name: String,
    arr: &ArrowFunctionExpression<'a>,
    exported: bool,
) {
    let start = line_at(st.source, arr.span.start);
    let end = line_at(st.source, arr.span.end);
    let (cc, nest, jsx, hooks) = body_metrics_jsx_hooks_arrow(arr);
    let (cognitive_complexity, clone_hash, param_count) = if arr.expression {
        if let Some(expr) = arr.get_expression() {
            (
                cognitive_expression(expr),
                ts_expression_shape_hash(expr),
                count_ts_formal_params(&arr.params),
            )
        } else {
            (0, 0, count_ts_formal_params(&arr.params))
        }
    } else {
        (
            cognitive_function_body(&arr.body),
            ts_function_body_shape_hash(&arr.body),
            count_ts_formal_params(&arr.params),
        )
    };
    let exact_clone_hash = exact_hash_from_span(st.source, arr.span.start, arr.span.end);
    let is_comp = st.is_tsx && is_component_name(&name) && jsx;
    let props = if is_comp {
        extract_props_arrow(arr)
    } else {
        vec![]
    };
    let hooks = if is_comp { hooks } else { vec![] };
    st.functions.push(TsFuncInfo {
        name: name.clone(),
        file: st.file.to_string(),
        line: start,
        end_line: end,
        line_count: end.saturating_sub(start) + 1,
        complexity: cc,
        cognitive_complexity,
        nesting: nest,
        param_count,
        clone_hash,
        exact_clone_hash,
        exported,
        is_component: is_comp,
        props,
        hooks,
        is_test: st.is_test_file,
    });
    if exported {
        exports.push(name);
    }
}

fn file_imports_mobx_observer(imports: &[TsImportInfo]) -> bool {
    imports.iter().any(|i| {
        (i.source == "mobx-react-lite" || i.source == "mobx-react")
            && i.specifiers
                .iter()
                .any(|s| s == "observer" || s.contains("observer"))
    })
}

fn collect_mobx_issues(
    program: &Program<'_>,
    rel: &str,
    source: &str,
    imports: &[TsImportInfo],
    is_tsx: bool,
) -> Vec<MobxObserverInfo> {
    if !is_tsx || !file_imports_mobx_observer(imports) {
        return vec![];
    }
    let mut out = Vec::new();
    for stmt in &program.body {
        match stmt {
            Statement::ExportNamedDeclaration(ex) => {
                if let Some(Declaration::FunctionDeclaration(f)) = &ex.declaration {
                    mobx_check_exported_function(f, rel, source, &mut out);
                }
                if let Some(Declaration::VariableDeclaration(v)) = &ex.declaration {
                    mobx_check_exported_vars(v, rel, source, &mut out);
                }
            }
            Statement::ExportDefaultDeclaration(ex) => {
                if let ExportDefaultDeclarationKind::FunctionDeclaration(f) = &ex.declaration {
                    mobx_check_exported_function(f, rel, source, &mut out);
                }
            }
            _ => {}
        }
    }
    out
}

fn mobx_check_exported_function(
    f: &Function<'_>,
    rel: &str,
    source: &str,
    out: &mut Vec<MobxObserverInfo>,
) {
    let Some(id) = &f.id else { return };
    let name = id.name.as_str().to_string();
    if !is_component_name(&name) {
        return;
    }
    let Some(body) = &f.body else { return };
    if !body_contains_jsx(body) {
        return;
    }
    out.push(MobxObserverInfo {
        file: rel.to_string(),
        line: line_at(source, f.span.start),
        component: name,
        kind: "exported function component not wrapped in observer()".to_string(),
    });
}

fn mobx_check_exported_vars(
    v: &VariableDeclaration<'_>,
    rel: &str,
    source: &str,
    out: &mut Vec<MobxObserverInfo>,
) {
    for decl in &v.declarations {
        let BindingPattern::BindingIdentifier(id) = &decl.id else {
            continue;
        };
        let name = id.name.as_str().to_string();
        if !is_component_name(&name) {
            continue;
        }
        let Some(init) = decl.init.as_ref() else {
            continue;
        };
        if let Expression::CallExpression(call) = init {
            if let Expression::Identifier(callee) = &call.callee {
                if callee.name.as_str() == "observer" {
                    continue;
                }
            }
        }
        let is_fn = matches!(
            init,
            Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_)
        );
        if !is_fn {
            continue;
        }
        let jsx = match init {
            Expression::ArrowFunctionExpression(a) => {
                if a.expression {
                    a.get_expression().map(expr_contains_jsx).unwrap_or(false)
                } else {
                    body_contains_jsx(&a.body)
                }
            }
            Expression::FunctionExpression(fe) => fe
                .body
                .as_ref()
                .map(|b| body_contains_jsx(b.as_ref()))
                .unwrap_or(false),
            _ => false,
        };
        if jsx {
            out.push(MobxObserverInfo {
                file: rel.to_string(),
                line: line_at(source, decl.span.start),
                component: name,
                kind: "exported component not wrapped in observer()".to_string(),
            });
        }
    }
}

fn process_class_declaration(
    c: &Class<'_>,
    file_src: &str,
    rel: &str,
    exported: bool,
    ts_st: &mut TsFuncScanState<'_>,
    exports: &mut Vec<String>,
    classes: &mut Vec<TsClassInfo>,
) {
    let Some(id) = &c.id else { return };
    let name = id.name.as_str().to_string();
    if exported {
        exports.push(name.clone());
    }
    let start = line_at(file_src, c.span.start);
    let end = line_at(file_src, c.span.end);
    let mut methods = 0usize;
    let mut properties = 0usize;
    for el in &c.body.body {
        match el {
            ClassElement::MethodDefinition(m) => {
                methods += 1;
                if matches!(
                    m.kind,
                    MethodDefinitionKind::Method | MethodDefinitionKind::Constructor
                ) {
                    if let Some(mname) = property_key_method_name(&m.key) {
                        process_function(ts_st, exports, mname, &m.value, false);
                    }
                }
            }
            ClassElement::PropertyDefinition(_) => properties += 1,
            ClassElement::AccessorProperty(_) => properties += 1,
            _ => {}
        }
    }
    classes.push(TsClassInfo {
        name,
        file: rel.to_string(),
        line: start,
        line_count: end.saturating_sub(start) + 1,
        methods,
        properties,
        exported,
        has_heritage: c.super_class.is_some(),
    });
}

fn process_variable_decl(
    v: &VariableDeclaration<'_>,
    exported: bool,
    ts_st: &mut TsFuncScanState<'_>,
    exports: &mut Vec<String>,
) {
    for decl in &v.declarations {
        if let BindingPattern::BindingIdentifier(id) = &decl.id {
            let name = id.name.as_str().to_string();
            if let Some(Expression::ArrowFunctionExpression(arr)) = &decl.init {
                process_arrow_var(ts_st, exports, name, arr, exported);
            } else if let Some(Expression::FunctionExpression(fe)) = &decl.init {
                process_function(ts_st, exports, name.clone(), fe, exported);
            } else if exported {
                exports.push(name);
            }
        }
    }
}

struct FileCtx<'a, 'b> {
    abs: &'a Path,
    scan_root: &'a Path,
    alias_prefix: &'a str,
    file_src: &'a str,
    rel: &'a str,
    ts_st: &'a mut TsFuncScanState<'b>,
    exports: &'a mut Vec<String>,
    imports: &'a mut Vec<TsImportInfo>,
    classes: &'a mut Vec<TsClassInfo>,
}

fn process_export_named(ex: &ExportNamedDeclaration<'_>, ctx: &mut FileCtx<'_, '_>) {
    if let Some(decl) = &ex.declaration {
        match decl {
            Declaration::FunctionDeclaration(f) => {
                if let Some(id) = &f.id {
                    process_function(ctx.ts_st, ctx.exports, id.name.as_str().to_string(), f, true);
                }
            }
            Declaration::VariableDeclaration(v) => {
                process_variable_decl(v, true, ctx.ts_st, ctx.exports);
            }
            Declaration::ClassDeclaration(c) => {
                process_class_declaration(c, ctx.file_src, ctx.rel, true, ctx.ts_st, ctx.exports, ctx.classes);
            }
            Declaration::TSTypeAliasDeclaration(t) => {
                ctx.exports.push(t.id.name.as_str().to_string());
            }
            Declaration::TSInterfaceDeclaration(t) => {
                ctx.exports.push(t.id.name.as_str().to_string());
            }
            Declaration::TSEnumDeclaration(e) => {
                ctx.exports.push(e.id.name.as_str().to_string());
            }
            _ => {}
        }
    }
    if let Some(src) = &ex.source {
        let imp_path = src.value.as_str();
        let (is_internal, resolved) = resolve_import(imp_path, ctx.abs, ctx.scan_root, ctx.alias_prefix);
        let mut re_specs = Vec::new();
        for spec in &ex.specifiers {
            re_specs.push(module_export_name_str(&spec.local));
            ctx.exports.push(module_export_name_str(&spec.exported));
        }
        ctx.imports.push(TsImportInfo {
            source: imp_path.to_string(),
            specifiers: re_specs,
            is_internal,
            resolved_path: resolved,
            line: line_at(ctx.file_src, ex.span.start),
        });
    } else {
        for spec in &ex.specifiers {
            ctx.exports.push(module_export_name_str(&spec.exported));
        }
    }
}

pub(crate) fn analyze_ts_file(
    abs: &Path,
    scan_root: &Path,
    alias_prefix: &str,
    orm_methods: Option<&HashSet<String>>,
    exclude: &[String],
) -> Option<TsFileData> {
    let _ = exclude;
    let abs = abs.canonicalize().unwrap_or_else(|_| abs.to_path_buf());
    let source = fs::read_to_string(&abs).ok()?;
    let rel = display_rel(&abs, scan_root);
    let alloc = Allocator::default();
    let source_type = SourceType::from_path(&abs).ok()?;
    let ret = Parser::new(&alloc, source.as_str(), source_type).parse();
    if !ret.errors.is_empty() {
        eprintln!("  SKIP: {rel}: {} parse error(s)", ret.errors.len());
        return None;
    }
    let program = &ret.program;
    let file_src = source.as_str();
    let is_tsx = abs
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("tsx") || e.eq_ignore_ascii_case("jsx"));

    let line_count = source.lines().count().max(1);
    let mut functions = Vec::new();
    let mut classes = Vec::new();
    let mut imports = Vec::new();
    let mut exports = Vec::new();
    let mut star_reexport_sources = Vec::new();
    let is_test_file = is_ts_test_path(&rel);

    let mut ts_st = TsFuncScanState {
        functions: &mut functions,
        file: rel.as_str(),
        source: file_src,
        is_tsx,
        is_test_file,
    };

    for stmt in &program.body {
        match stmt {
            Statement::ImportDeclaration(im) => {
                ingest_import_declaration(im, &abs, scan_root, alias_prefix, file_src, &mut imports);
            }
            Statement::ExportAllDeclaration(ex) => {
                let imp_path = ex.source.value.as_str();
                let (is_internal, resolved) = resolve_import(imp_path, &abs, scan_root, alias_prefix);
                if is_internal {
                    star_reexport_sources.push(resolved);
                }
            }
            Statement::ExportNamedDeclaration(ex) => {
                let mut ctx = FileCtx {
                    abs: &abs, scan_root, alias_prefix, file_src, rel: &rel,
                    ts_st: &mut ts_st, exports: &mut exports,
                    imports: &mut imports, classes: &mut classes,
                };
                process_export_named(ex, &mut ctx);
            }
            Statement::ExportDefaultDeclaration(ex) => {
                match &ex.declaration {
                    ExportDefaultDeclarationKind::FunctionDeclaration(f) => {
                        if let Some(id) = &f.id {
                            process_function(&mut ts_st, &mut exports, id.name.as_str().to_string(), f, true);
                        }
                    }
                    ExportDefaultDeclarationKind::ClassDeclaration(c) => {
                        process_class_declaration(c, file_src, &rel, true, &mut ts_st, &mut exports, &mut classes);
                    }
                    _ => {}
                }
            }
            Statement::FunctionDeclaration(f) => {
                if let Some(id) = &f.id {
                    process_function(&mut ts_st, &mut exports, id.name.as_str().to_string(), f, false);
                }
            }
            Statement::VariableDeclaration(v) => {
                process_variable_decl(v, false, &mut ts_st, &mut exports);
            }
            Statement::ClassDeclaration(c) => {
                process_class_declaration(c, file_src, &rel, false, &mut ts_st, &mut exports, &mut classes);
            }
            Statement::TSTypeAliasDeclaration(_)
            | Statement::TSInterfaceDeclaration(_)
            | Statement::TSEnumDeclaration(_) => {}
            _ => {}
        }
    }

    let any_count = count_any_in_program(program);
    let console_debugger = collect_console_debugger(program, &rel, &source);
    let silent_catches = collect_silent_catches(program, &rel, &source);
    let mobx_observer_issues = collect_mobx_issues(program, &rel, &source, &imports, is_tsx);
    let orm_case_issues = if let Some(m) = orm_methods {
        if !m.is_empty() {
            collect_orm_case_findings(program, &source, &rel, m)
        } else {
            vec![]
        }
    } else {
        vec![]
    };

    let mut security_findings = Vec::new();
    collect_ts_security(program, &rel, file_src, &mut security_findings);

    let namespace_import_sources: Vec<String> = imports
        .iter()
        .filter(|i| i.is_internal && i.specifiers.iter().any(|s| s.starts_with("* as ")))
        .map(|i| i.resolved_path.clone())
        .collect();

    Some(TsFileData {
        rel_path: rel,
        abs_path: abs.display().to_string(),
        line_count,
        functions,
        classes,
        imports,
        exports,
        star_reexport_sources,
        namespace_import_sources,
        source,
        any_count,
        console_debugger,
        silent_catches,
        mobx_observer_issues,
        orm_case_issues,
        security_findings,
        is_test_file,
    })
}

pub(crate) fn check_boundaries(
    all: &[TsFileData],
    rules: &[BoundaryRule],
) -> Vec<BoundaryViolation> {
    let mut violations = Vec::new();
    for d in all {
        let matched: Vec<_> = rules
            .iter()
            .filter(|r| d.rel_path.starts_with(&r.source))
            .collect();
        if matched.is_empty() {
            continue;
        }
        let mut import_line: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for imp in &d.imports {
            import_line.insert(imp.source.clone(), imp.line);
        }
        for rule in &matched {
            for imp in &d.imports {
                for fp in &rule.forbidden {
                    if imp.source.starts_with(fp) {
                        violations.push(BoundaryViolation {
                            file: d.rel_path.clone(),
                            line: *import_line.get(&imp.source).unwrap_or(&1),
                            import_source: imp.source.clone(),
                            rule: format!("{} -> {}", rule.source, fp),
                        });
                    }
                }
            }
        }
    }
    violations.sort_by(|a, b| a.file.cmp(&b.file).then_with(|| a.line.cmp(&b.line)));
    violations
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn resolve_import_alias_prefix() {
        let root = Path::new("/repo/src");
        let from = root.join("app/page.tsx");
        let (internal, res) = resolve_import("@/ui/button", &from, root, "@/");
        assert!(internal);
        assert_eq!(res, "ui/button");
    }

    #[test]
    fn resolve_import_relative() {
        let root = Path::new("/repo/src");
        let from = root.join("app/page.tsx");
        let (internal, res) = resolve_import("./helper", &from, root, "@/");
        assert!(internal);
        assert_eq!(res, "app/helper");
    }

    #[test]
    fn normalize_module_path_strips_extensions_and_index() {
        assert_eq!(normalize_module_path("foo/bar.ts"), "foo/bar");
        assert_eq!(normalize_module_path("x/Button.tsx"), "x/Button");
        assert_eq!(normalize_module_path("pkg/index.js"), "pkg");
    }

    #[test]
    fn collect_ts_files_skips_node_modules() {
        let base = std::env::temp_dir().join(format!("ast-scan-ts-collect-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("src")).unwrap();
        fs::write(base.join("src/ok.ts"), "export const x = 1").unwrap();
        fs::create_dir_all(base.join("src/node_modules/pkg")).unwrap();
        fs::write(base.join("src/node_modules/pkg/nope.ts"), "").unwrap();
        let files = collect_ts_files(&base.join("src"), &[]);
        let _ = fs::remove_dir_all(&base);
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("ok.ts"));
    }
}
