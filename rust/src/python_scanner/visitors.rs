//! Python AST visitors: complexity, nesting, dependency collection, silent handlers.

use crate::types::{ImportEdge, RouteInfo};

use rustpython_parser::{
    ast::Ranged,
    ast::{
        self, Arg, ArgWithDefault, Arguments, Comprehension, Constant, ExceptHandler, Expr, Stmt,
    },
    text_size::TextSize,
};

pub(super) fn line_at(source: &str, pos: TextSize) -> usize {
    let o = usize::from(pos);
    if o > source.len() {
        return 1;
    }
    source[..o].bytes().filter(|&b| b == b'\n').count() + 1
}

pub(super) fn line_at_end(source: &str, pos: TextSize) -> usize {
    let o = usize::from(pos);
    if o > source.len() {
        return line_at(source, pos);
    }
    source[..o].bytes().filter(|&b| b == b'\n').count() + 1
}
fn const_value_str(expr: &Expr) -> String {
    match expr {
        Expr::Constant(c) => match &c.value {
            Constant::Str(s) => format!("{s:?}"),
            Constant::Int(i) => i.to_string(),
            Constant::Float(f) => f.to_string(),
            Constant::Bool(b) => b.to_string(),
            Constant::None => "None".into(),
            _ => format!("{expr:?}"),
        },
        _ => format!("{expr:?}"),
    }
}

fn decorator_name(expr: &Expr) -> String {
    match expr {
        Expr::Name(n) => n.id.to_string(),
        Expr::Attribute(a) => format!("{}.{}", decorator_name(&a.value), a.attr),
        Expr::Call(c) => decorator_name(&c.func),
        _ => format!("{expr:?}"),
    }
}

pub(super) fn decorator_repr(expr: &Expr) -> String {
    if let Expr::Call(c) = expr {
        let fname = decorator_name(&c.func);
        let mut args: Vec<String> = c.args.iter().map(const_value_str).collect();
        for kw in &c.keywords {
            let val = const_value_str(&kw.value);
            if let Some(arg) = &kw.arg {
                args.push(format!("{}={}", arg, val));
            } else {
                args.push(val);
            }
        }
        format!("@{}({})", fname, args.join(", "))
    } else {
        format!("@{}", decorator_name(expr))
    }
}

// ---------------------------------------------------------------------------
// Generic AST walk helpers — enumerate children once, used by all visitors.
// ---------------------------------------------------------------------------

fn for_each_comp_expr(c: &Comprehension, f: &mut impl FnMut(&Expr)) {
    f(&c.target);
    f(&c.iter);
    for iff in &c.ifs {
        f(iff);
    }
}

fn visit_elt_generators(elt: &Expr, generators: &[Comprehension], f: &mut impl FnMut(&Expr)) {
    f(elt);
    for g in generators {
        for_each_comp_expr(g, f);
    }
}

fn visit_elts(elts: &[Expr], f: &mut impl FnMut(&Expr)) {
    for e in elts {
        f(e);
    }
}

/// Call `f` for every direct child expression inside `expr`.
fn for_each_child_expr(expr: &Expr, f: &mut impl FnMut(&Expr)) {
    match expr {
        Expr::BoolOp(b) => visit_elts(&b.values, f),
        Expr::NamedExpr(n) => {
            f(&n.target);
            f(&n.value);
        }
        Expr::BinOp(b) => {
            f(&b.left);
            f(&b.right);
        }
        Expr::UnaryOp(u) => f(&u.operand),
        Expr::Lambda(l) => f(&l.body),
        Expr::IfExp(i) => {
            f(&i.test);
            f(&i.body);
            f(&i.orelse);
        }
        Expr::Dict(d) => {
            for (k, v) in d.keys.iter().zip(&d.values) {
                if let Some(k) = k {
                    f(k);
                }
                f(v);
            }
        }
        Expr::Set(s) => visit_elts(&s.elts, f),
        Expr::ListComp(l) => visit_elt_generators(&l.elt, &l.generators, f),
        Expr::SetComp(s) => visit_elt_generators(&s.elt, &s.generators, f),
        Expr::DictComp(d) => {
            f(&d.key);
            f(&d.value);
            for g in &d.generators {
                for_each_comp_expr(g, f);
            }
        }
        Expr::GeneratorExp(g) => visit_elt_generators(&g.elt, &g.generators, f),
        Expr::Await(a) => f(&a.value),
        Expr::Yield(y) => {
            if let Some(v) = &y.value {
                f(v);
            }
        }
        Expr::YieldFrom(y) => f(&y.value),
        Expr::Compare(c) => {
            f(&c.left);
            for cm in &c.comparators {
                f(cm);
            }
        }
        Expr::Call(c) => {
            f(&c.func);
            visit_elts(&c.args, f);
            for kw in &c.keywords {
                f(&kw.value);
            }
        }
        Expr::FormattedValue(fv) => {
            f(&fv.value);
            if let Some(spec) = &fv.format_spec {
                f(spec);
            }
        }
        Expr::JoinedStr(j) => visit_elts(&j.values, f),
        Expr::Attribute(a) => f(&a.value),
        Expr::Subscript(s) => {
            f(&s.value);
            f(&s.slice);
        }
        Expr::Starred(s) => f(&s.value),
        Expr::List(l) => visit_elts(&l.elts, f),
        Expr::Tuple(t) => visit_elts(&t.elts, f),
        Expr::Slice(s) => {
            if let Some(l) = &s.lower {
                f(l);
            }
            if let Some(u) = &s.upper {
                f(u);
            }
            if let Some(st) = &s.step {
                f(st);
            }
        }
        _ => {}
    }
}

fn visit_body(body: &[Stmt], f: &mut impl FnMut(&Stmt)) {
    for st in body {
        f(st);
    }
}

fn visit_body_orelse(body: &[Stmt], orelse: &[Stmt], f: &mut impl FnMut(&Stmt)) {
    visit_body(body, f);
    visit_body(orelse, f);
}

fn visit_try_bodies(
    body: &[Stmt],
    orelse: &[Stmt],
    finalbody: &[Stmt],
    f: &mut impl FnMut(&Stmt),
) {
    visit_body(body, f);
    visit_body(orelse, f);
    visit_body(finalbody, f);
}

/// Call `f` for every direct child statement.  Handler bodies are NOT visited
/// -- use `for_each_handler` separately so the borrow checker is happy.
fn for_each_child_stmt(stmt: &Stmt, f: &mut impl FnMut(&Stmt)) {
    match stmt {
        Stmt::FunctionDef(fd) => visit_body(&fd.body, f),
        Stmt::AsyncFunctionDef(fd) => visit_body(&fd.body, f),
        Stmt::ClassDef(c) => visit_body(&c.body, f),
        Stmt::If(s) => visit_body_orelse(&s.body, &s.orelse, f),
        Stmt::For(s) => visit_body_orelse(&s.body, &s.orelse, f),
        Stmt::AsyncFor(s) => visit_body_orelse(&s.body, &s.orelse, f),
        Stmt::While(s) => visit_body_orelse(&s.body, &s.orelse, f),
        Stmt::With(s) => visit_body(&s.body, f),
        Stmt::AsyncWith(s) => visit_body(&s.body, f),
        Stmt::Try(s) => visit_try_bodies(&s.body, &s.orelse, &s.finalbody, f),
        Stmt::TryStar(s) => visit_try_bodies(&s.body, &s.orelse, &s.finalbody, f),
        Stmt::Match(m) => {
            for c in &m.cases {
                visit_body(&c.body, f);
            }
        }
        _ => {}
    }
}

/// Call `f` for every exception handler in a Try / TryStar statement.
fn for_each_handler(stmt: &Stmt, f: &mut impl FnMut(&ExceptHandler)) {
    match stmt {
        Stmt::Try(s) => {
            for h in &s.handlers {
                f(h);
            }
        }
        Stmt::TryStar(s) => {
            for h in &s.handlers {
                f(h);
            }
        }
        _ => {}
    }
}

/// Call `f` for every direct child expression of a statement (test, target,
/// iter, value, etc.).  Does NOT include handler types — those belong to the
/// handler callback.
fn visit_with_items(items: &[ast::WithItem], f: &mut impl FnMut(&Expr)) {
    for it in items {
        f(&it.context_expr);
        if let Some(v) = &it.optional_vars {
            f(v);
        }
    }
}

/// Call `f` for every direct child expression of a statement (test, target,
/// iter, value, etc.).  Does NOT include handler types -- those belong to the
/// handler callback.
fn for_each_expr_in_stmt(stmt: &Stmt, f: &mut impl FnMut(&Expr)) {
    match stmt {
        Stmt::If(s) => f(&s.test),
        Stmt::For(s) => {
            f(&s.target);
            f(&s.iter);
        }
        Stmt::AsyncFor(s) => {
            f(&s.target);
            f(&s.iter);
        }
        Stmt::While(s) => f(&s.test),
        Stmt::With(s) => visit_with_items(&s.items, f),
        Stmt::AsyncWith(s) => visit_with_items(&s.items, f),
        Stmt::Assign(a) => {
            visit_elts(&a.targets, f);
            f(&a.value);
        }
        Stmt::AnnAssign(a) => {
            f(&a.target);
            f(&a.annotation);
            if let Some(v) = &a.value {
                f(v);
            }
        }
        Stmt::AugAssign(a) => {
            f(&a.target);
            f(&a.value);
        }
        Stmt::Return(r) => {
            if let Some(v) = &r.value {
                f(v);
            }
        }
        Stmt::Delete(d) => visit_elts(&d.targets, f),
        Stmt::Expr(e) => f(&e.value),
        Stmt::Assert(s) => {
            f(&s.test);
            if let Some(m) = &s.msg {
                f(m);
            }
        }
        Stmt::Match(m) => {
            f(&m.subject);
            for c in &m.cases {
                if let Some(g) = &c.guard {
                    f(g);
                }
            }
        }
        Stmt::ClassDef(c) => {
            visit_elts(&c.decorator_list, f);
            visit_elts(&c.bases, f);
        }
        Stmt::Raise(r) => {
            if let Some(exc) = &r.exc {
                f(exc);
            }
            if let Some(c) = &r.cause {
                f(c);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Cyclomatic complexity
// ---------------------------------------------------------------------------

fn complexity_simple_arg(a: &Arg, cc: &mut usize) {
    if let Some(ann) = &a.annotation {
        complexity_expr(ann, cc);
    }
}

fn complexity_arg_with_default(a: &ArgWithDefault, cc: &mut usize) {
    complexity_simple_arg(&a.def, cc);
    if let Some(d) = &a.default {
        complexity_expr(d, cc);
    }
}

fn complexity_function_like(args: &Arguments, decorators: &[Expr], body: &[Stmt], cc: &mut usize) {
    for d in decorators {
        complexity_expr(d, cc);
    }
    for a in &args.posonlyargs {
        complexity_arg_with_default(a, cc);
    }
    for a in &args.args {
        complexity_arg_with_default(a, cc);
    }
    if let Some(v) = &args.vararg {
        complexity_simple_arg(v, cc);
    }
    for a in &args.kwonlyargs {
        complexity_arg_with_default(a, cc);
    }
    if let Some(k) = &args.kwarg {
        complexity_simple_arg(k, cc);
    }
    for st in body {
        complexity_stmt(st, cc);
    }
}

fn complexity_stmt(stmt: &Stmt, cc: &mut usize) {
    match stmt {
        Stmt::FunctionDef(f) => {
            complexity_function_like(&f.args, &f.decorator_list, &f.body, cc);
            return;
        }
        Stmt::AsyncFunctionDef(f) => {
            complexity_function_like(&f.args, &f.decorator_list, &f.body, cc);
            return;
        }
        Stmt::ClassDef(c) => {
            for d in &c.decorator_list {
                complexity_expr(d, cc);
            }
            for b in &c.bases {
                complexity_expr(b, cc);
            }
            for st in &c.body {
                complexity_stmt(st, cc);
            }
            return;
        }
        Stmt::Match(m) => {
            complexity_expr(&m.subject, cc);
            for case in &m.cases {
                if let Some(g) = &case.guard {
                    complexity_expr(g, cc);
                }
                for st in &case.body {
                    complexity_stmt(st, cc);
                }
            }
            return;
        }
        _ => {}
    }
    if matches!(
        stmt,
        Stmt::If(_)
            | Stmt::For(_)
            | Stmt::AsyncFor(_)
            | Stmt::While(_)
            | Stmt::With(_)
            | Stmt::AsyncWith(_)
            | Stmt::Assert(_)
    ) {
        *cc += 1;
    }
    for_each_child_stmt(stmt, &mut |child| complexity_stmt(child, cc));
    for_each_handler(stmt, &mut |h| complexity_except_handler(h, cc));
    for_each_expr_in_stmt(stmt, &mut |e| complexity_expr(e, cc));
}

fn complexity_except_handler(h: &ExceptHandler, cc: &mut usize) {
    *cc += 1;
    let ExceptHandler::ExceptHandler(eh) = h;
    if let Some(ty) = &eh.type_ {
        complexity_expr(ty, cc);
    }
    for st in &eh.body {
        complexity_stmt(st, cc);
    }
}

fn complexity_comprehension(c: &Comprehension, cc: &mut usize) {
    *cc += 1 + c.ifs.len();
    complexity_expr(&c.target, cc);
    complexity_expr(&c.iter, cc);
    for iff in &c.ifs {
        complexity_expr(iff, cc);
    }
}

fn complexity_expr(expr: &Expr, cc: &mut usize) {
    match expr {
        Expr::BoolOp(b) => *cc += b.values.len().saturating_sub(1),
        Expr::Lambda(l) => {
            for a in &l.args.posonlyargs {
                complexity_arg_with_default(a, cc);
            }
            for a in &l.args.args {
                complexity_arg_with_default(a, cc);
            }
            if let Some(v) = &l.args.vararg {
                complexity_simple_arg(v, cc);
            }
            for a in &l.args.kwonlyargs {
                complexity_arg_with_default(a, cc);
            }
            if let Some(k) = &l.args.kwarg {
                complexity_simple_arg(k, cc);
            }
            complexity_expr(&l.body, cc);
            return;
        }
        Expr::ListComp(l) => {
            complexity_expr(&l.elt, cc);
            for g in &l.generators {
                complexity_comprehension(g, cc);
            }
            return;
        }
        Expr::SetComp(s) => {
            complexity_expr(&s.elt, cc);
            for g in &s.generators {
                complexity_comprehension(g, cc);
            }
            return;
        }
        Expr::DictComp(d) => {
            complexity_expr(&d.key, cc);
            complexity_expr(&d.value, cc);
            for g in &d.generators {
                complexity_comprehension(g, cc);
            }
            return;
        }
        Expr::GeneratorExp(g) => {
            complexity_expr(&g.elt, cc);
            for gen in &g.generators {
                complexity_comprehension(gen, cc);
            }
            return;
        }
        _ => {}
    }
    for_each_child_expr(expr, &mut |child| complexity_expr(child, cc));
}

pub(super) fn compute_complexity(body: &[Stmt]) -> usize {
    let mut cc = 1usize;
    for st in body {
        complexity_stmt(st, &mut cc);
    }
    cc
}

// ---------------------------------------------------------------------------
// Nesting depth
// ---------------------------------------------------------------------------

fn nesting_stmt(stmt: &Stmt, depth: usize, max_d: &mut usize) {
    let d = if matches!(
        stmt,
        Stmt::If(_)
            | Stmt::For(_)
            | Stmt::AsyncFor(_)
            | Stmt::While(_)
            | Stmt::With(_)
            | Stmt::AsyncWith(_)
            | Stmt::Try(_)
            | Stmt::TryStar(_)
    ) {
        let nd = depth + 1;
        *max_d = (*max_d).max(nd);
        nd
    } else {
        depth
    };

    for_each_child_stmt(stmt, &mut |child| nesting_stmt(child, d, max_d));
    for_each_handler(stmt, &mut |h| {
        let ExceptHandler::ExceptHandler(eh) = h;
        let nd = d + 1;
        *max_d = (*max_d).max(nd);
        for st in &eh.body {
            nesting_stmt(st, nd, max_d);
        }
    });
}

pub(super) fn compute_max_nesting(body: &[Stmt]) -> usize {
    let mut max_d = 0usize;
    for st in body {
        nesting_stmt(st, 0, &mut max_d);
    }
    max_d
}

// ---------------------------------------------------------------------------
// Import / route / dependency collection
// ---------------------------------------------------------------------------

fn resolve_import_from(module: &str, level: usize, current_module: &str) -> String {
    if level == 0 {
        return module.to_string();
    }
    let parts: Vec<&str> = current_module.split('.').collect();
    let base_len = parts.len().saturating_sub(level).max(1);
    let base = parts[..base_len].join(".");
    if module.is_empty() {
        base
    } else {
        format!("{base}.{module}")
    }
}

fn internal_module(mod_name: &str, pkg: &str) -> bool {
    mod_name == pkg || mod_name.starts_with(&format!("{pkg}."))
}

pub(super) fn process_import(stmt: &Stmt, module: &str, pkg: &str, out: &mut Vec<ImportEdge>) {
    match stmt {
        Stmt::Import(i) => {
            for al in &i.names {
                let m = al.name.to_string();
                if internal_module(&m, pkg) {
                    let n = al
                        .asname
                        .as_ref()
                        .map(|a: &ast::Identifier| a.to_string())
                        .unwrap_or_else(|| al.name.to_string());
                    out.push(ImportEdge {
                        source_module: module.to_string(),
                        target_module: m,
                        names: vec![n],
                    });
                }
            }
        }
        Stmt::ImportFrom(i) => {
            let Some(mid) = &i.module else { return };
            let level = i
                .level
                .as_ref()
                .map(|l: &ast::Int| l.to_usize())
                .unwrap_or(0);
            let resolved = resolve_import_from(mid.as_str(), level, module);
            if internal_module(&resolved, pkg) {
                let names: Vec<String> = i.names.iter().map(|a| a.name.to_string()).collect();
                out.push(ImportEdge {
                    source_module: module.to_string(),
                    target_module: resolved,
                    names,
                });
            }
        }
        _ => {}
    }
}

fn collect_deps_from_body(body: &[Stmt], out: &mut Vec<String>) {
    for st in body {
        collect_deps_stmt(st, out);
    }
}

fn collect_deps_stmt(stmt: &Stmt, out: &mut Vec<String>) {
    for_each_child_stmt(stmt, &mut |child| collect_deps_stmt(child, out));
    for_each_handler(stmt, &mut |h| {
        let ExceptHandler::ExceptHandler(eh) = h;
        for st in &eh.body {
            collect_deps_stmt(st, out);
        }
    });
    for_each_expr_in_stmt(stmt, &mut |e| collect_deps_expr(e, out));
}

fn collect_deps_expr(expr: &Expr, out: &mut Vec<String>) {
    if let Expr::Call(c) = expr {
        if let Expr::Name(n) = c.func.as_ref() {
            if n.id.as_str() == "Depends" && !c.args.is_empty() {
                out.push(format!("{:?}", c.args[0]));
            }
        }
    }
    for_each_child_expr(expr, &mut |child| collect_deps_expr(child, out));
}

pub(super) fn extract_route(
    _name: &str,
    qualname: &str,
    filepath: &str,
    line: usize,
    decorators: &[Expr],
    body: &[Stmt],
) -> Option<RouteInfo> {
    let http = ["get", "post", "put", "delete", "patch", "head", "options"];
    for dec in decorators {
        let Expr::Call(c) = dec else { continue };
        let Expr::Attribute(attr) = c.func.as_ref() else {
            continue;
        };
        let m = attr.attr.as_str();
        if !http.contains(&m) {
            continue;
        }
        let method = m.to_uppercase();
        let mut path = String::new();
        if let Some(Expr::Constant(co)) = c.args.first() {
            if let Constant::Str(s) = &co.value {
                path = s.clone();
            }
        }
        let mut deps = Vec::new();
        collect_deps_from_body(body, &mut deps);
        return Some(RouteInfo {
            method,
            path,
            handler: qualname.to_string(),
            file: filepath.to_string(),
            line,
            dependencies: deps,
        });
    }
    None
}

// ---------------------------------------------------------------------------
// Silent catch detection
// ---------------------------------------------------------------------------

pub(super) fn collect_silent_excepts(
    stmts: &[Stmt],
    filepath: &str,
    source: &str,
    out: &mut Vec<crate::types::SilentCatchInfo>,
) {
    for stmt in stmts {
        collect_silent_stmt(stmt, filepath, source, out);
    }
}

fn check_silent_handler(
    h: &ExceptHandler,
    filepath: &str,
    source: &str,
    out: &mut Vec<crate::types::SilentCatchInfo>,
) {
    let ExceptHandler::ExceptHandler(eh) = h;
    let line = line_at(source, eh.range().start());
    let body = &eh.body;
    if body.is_empty() {
        out.push(crate::types::SilentCatchInfo {
            file: filepath.to_string(),
            line,
            kind: "except: <empty>".into(),
        });
    } else if body.len() == 1 {
        match &body[0] {
            Stmt::Pass(_) => {
                out.push(crate::types::SilentCatchInfo {
                    file: filepath.to_string(),
                    line,
                    kind: "except: pass".into(),
                });
            }
            Stmt::Expr(e) => {
                if let Expr::Constant(_) = e.value.as_ref() {
                    out.push(crate::types::SilentCatchInfo {
                        file: filepath.to_string(),
                        line,
                        kind: "except: <string-only>".into(),
                    });
                }
            }
            _ => {}
        }
    }
}

fn collect_silent_stmt(
    stmt: &Stmt,
    filepath: &str,
    source: &str,
    out: &mut Vec<crate::types::SilentCatchInfo>,
) {
    for_each_child_stmt(stmt, &mut |child| {
        collect_silent_stmt(child, filepath, source, out);
    });
    for_each_handler(stmt, &mut |h| {
        check_silent_handler(h, filepath, source, out);
        let ExceptHandler::ExceptHandler(eh) = h;
        for st in &eh.body {
            collect_silent_stmt(st, filepath, source, out);
        }
    });
}
