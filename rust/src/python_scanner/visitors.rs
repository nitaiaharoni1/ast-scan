//! Python AST visitors: complexity, nesting, dependency collection, silent handlers.

use crate::clones;
use crate::secrets;
use crate::types::{ImportEdge, RouteInfo, SecurityFinding};

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
// Cognitive complexity (nesting-weighted branching)
// ---------------------------------------------------------------------------

fn cog_comprehension(c: &Comprehension, nest: usize, acc: &mut usize) {
    *acc += 1 + nest + c.ifs.len() * (1 + nest);
    cog_expr(&c.target, nest, acc);
    cog_expr(&c.iter, nest, acc);
    for iff in &c.ifs {
        cog_expr(iff, nest, acc);
    }
}

fn cog_expr(expr: &Expr, nest: usize, acc: &mut usize) {
    match expr {
        Expr::BoolOp(b) => {
            *acc += b.values.len().saturating_sub(1) * (1 + nest);
            for v in &b.values {
                cog_expr(v, nest, acc);
            }
            return;
        }
        Expr::IfExp(i) => {
            *acc += 1 + nest;
            cog_expr(&i.test, nest, acc);
            cog_expr(&i.body, nest + 1, acc);
            cog_expr(&i.orelse, nest + 1, acc);
            return;
        }
        Expr::Lambda(l) => {
            for a in &l.args.posonlyargs {
                if let Some(ann) = &a.def.annotation {
                    cog_expr(ann, nest, acc);
                }
                if let Some(d) = &a.default {
                    cog_expr(d, nest, acc);
                }
            }
            for a in &l.args.args {
                if let Some(ann) = &a.def.annotation {
                    cog_expr(ann, nest, acc);
                }
                if let Some(d) = &a.default {
                    cog_expr(d, nest, acc);
                }
            }
            if let Some(v) = &l.args.vararg {
                if let Some(ann) = &v.annotation {
                    cog_expr(ann, nest, acc);
                }
            }
            for a in &l.args.kwonlyargs {
                if let Some(ann) = &a.def.annotation {
                    cog_expr(ann, nest, acc);
                }
                if let Some(d) = &a.default {
                    cog_expr(d, nest, acc);
                }
            }
            if let Some(k) = &l.args.kwarg {
                if let Some(ann) = &k.annotation {
                    cog_expr(ann, nest, acc);
                }
            }
            cog_expr(&l.body, nest, acc);
            return;
        }
        Expr::ListComp(l) => {
            cog_expr(&l.elt, nest, acc);
            for g in &l.generators {
                cog_comprehension(g, nest, acc);
            }
            return;
        }
        Expr::SetComp(s) => {
            cog_expr(&s.elt, nest, acc);
            for g in &s.generators {
                cog_comprehension(g, nest, acc);
            }
            return;
        }
        Expr::DictComp(d) => {
            cog_expr(&d.key, nest, acc);
            cog_expr(&d.value, nest, acc);
            for g in &d.generators {
                cog_comprehension(g, nest, acc);
            }
            return;
        }
        Expr::GeneratorExp(g) => {
            cog_expr(&g.elt, nest, acc);
            for gen in &g.generators {
                cog_comprehension(gen, nest, acc);
            }
            return;
        }
        _ => {}
    }
    for_each_child_expr(expr, &mut |child| cog_expr(child, nest, acc));
}

fn cog_except_handler(h: &ExceptHandler, nest: usize, acc: &mut usize) {
    *acc += 1 + nest;
    let ExceptHandler::ExceptHandler(eh) = h;
    if let Some(ty) = &eh.type_ {
        cog_expr(ty, nest + 1, acc);
    }
    for st in &eh.body {
        cog_stmt(st, nest + 1, acc);
    }
}

fn cog_function_like(args: &Arguments, decorators: &[Expr], body: &[Stmt], nest: usize, acc: &mut usize) {
    for d in decorators {
        cog_expr(d, nest, acc);
    }
    for a in &args.posonlyargs {
        if let Some(ann) = &a.def.annotation {
            cog_expr(ann, nest, acc);
        }
        if let Some(d) = &a.default {
            cog_expr(d, nest, acc);
        }
    }
    for a in &args.args {
        if let Some(ann) = &a.def.annotation {
            cog_expr(ann, nest, acc);
        }
        if let Some(d) = &a.default {
            cog_expr(d, nest, acc);
        }
    }
    if let Some(v) = &args.vararg {
        if let Some(ann) = &v.annotation {
            cog_expr(ann, nest, acc);
        }
    }
    for a in &args.kwonlyargs {
        if let Some(ann) = &a.def.annotation {
            cog_expr(ann, nest, acc);
        }
        if let Some(d) = &a.default {
            cog_expr(d, nest, acc);
        }
    }
    if let Some(k) = &args.kwarg {
        if let Some(ann) = &k.annotation {
            cog_expr(ann, nest, acc);
        }
    }
    for st in body {
        cog_stmt(st, nest, acc);
    }
}

fn cog_stmt(stmt: &Stmt, nest: usize, acc: &mut usize) {
    match stmt {
        Stmt::FunctionDef(f) => {
            cog_function_like(&f.args, &f.decorator_list, &f.body, nest, acc);
            return;
        }
        Stmt::AsyncFunctionDef(f) => {
            cog_function_like(&f.args, &f.decorator_list, &f.body, nest, acc);
            return;
        }
        Stmt::ClassDef(c) => {
            for d in &c.decorator_list {
                cog_expr(d, nest, acc);
            }
            for b in &c.bases {
                cog_expr(b, nest, acc);
            }
            for st in &c.body {
                cog_stmt(st, nest, acc);
            }
            return;
        }
        Stmt::If(i) => {
            *acc += 1 + nest;
            for st in &i.body {
                cog_stmt(st, nest + 1, acc);
            }
            for st in &i.orelse {
                cog_stmt(st, nest + 1, acc);
            }
            return;
        }
        Stmt::For(f) => {
            *acc += 1 + nest;
            cog_expr(&f.iter, nest + 1, acc);
            for st in &f.body {
                cog_stmt(st, nest + 1, acc);
            }
            for st in &f.orelse {
                cog_stmt(st, nest + 1, acc);
            }
            return;
        }
        Stmt::AsyncFor(f) => {
            *acc += 1 + nest;
            cog_expr(&f.iter, nest + 1, acc);
            for st in &f.body {
                cog_stmt(st, nest + 1, acc);
            }
            for st in &f.orelse {
                cog_stmt(st, nest + 1, acc);
            }
            return;
        }
        Stmt::While(w) => {
            *acc += 1 + nest;
            cog_expr(&w.test, nest + 1, acc);
            for st in &w.body {
                cog_stmt(st, nest + 1, acc);
            }
            for st in &w.orelse {
                cog_stmt(st, nest + 1, acc);
            }
            return;
        }
        Stmt::With(w) => {
            *acc += 1 + nest;
            for item in &w.items {
                cog_expr(&item.context_expr, nest + 1, acc);
                if let Some(v) = &item.optional_vars {
                    cog_expr(v, nest + 1, acc);
                }
            }
            for st in &w.body {
                cog_stmt(st, nest + 1, acc);
            }
            return;
        }
        Stmt::AsyncWith(w) => {
            *acc += 1 + nest;
            for item in &w.items {
                cog_expr(&item.context_expr, nest + 1, acc);
                if let Some(v) = &item.optional_vars {
                    cog_expr(v, nest + 1, acc);
                }
            }
            for st in &w.body {
                cog_stmt(st, nest + 1, acc);
            }
            return;
        }
        Stmt::Match(m) => {
            *acc += 1 + nest;
            cog_expr(&m.subject, nest + 1, acc);
            for case in &m.cases {
                if let Some(g) = &case.guard {
                    cog_expr(g, nest + 1, acc);
                }
                for st in &case.body {
                    cog_stmt(st, nest + 1, acc);
                }
            }
            return;
        }
        Stmt::Try(t) => {
            for st in &t.body {
                cog_stmt(st, nest + 1, acc);
            }
            for h in &t.handlers {
                cog_except_handler(h, nest, acc);
            }
            for st in &t.orelse {
                cog_stmt(st, nest + 1, acc);
            }
            for st in &t.finalbody {
                cog_stmt(st, nest + 1, acc);
            }
            return;
        }
        Stmt::TryStar(t) => {
            for st in &t.body {
                cog_stmt(st, nest + 1, acc);
            }
            for h in &t.handlers {
                cog_except_handler(h, nest, acc);
            }
            for st in &t.orelse {
                cog_stmt(st, nest + 1, acc);
            }
            for st in &t.finalbody {
                cog_stmt(st, nest + 1, acc);
            }
            return;
        }
        _ => {}
    }
    if matches!(stmt, Stmt::Assert(_)) {
        *acc += 1 + nest;
    }
    for_each_child_stmt(stmt, &mut |child| cog_stmt(child, nest, acc));
    for_each_handler(stmt, &mut |h| cog_except_handler(h, nest, acc));
    for_each_expr_in_stmt(stmt, &mut |e| cog_expr(e, nest, acc));
}

pub(super) fn compute_cognitive_complexity(body: &[Stmt]) -> usize {
    let mut acc = 0usize;
    for st in body {
        cog_stmt(st, 0, &mut acc);
    }
    acc
}

pub(super) fn count_python_params(args: &Arguments, is_method: bool) -> usize {
    let mut n = args.posonlyargs.len() + args.args.len();
    if is_method && !args.args.is_empty() {
        let id = args.args[0].def.arg.as_str();
        if id == "self" || id == "cls" {
            n = n.saturating_sub(1);
        }
    }
    n += args.vararg.is_some() as usize;
    n += args.kwonlyargs.len();
    n += args.kwarg.is_some() as usize;
    n
}

// ---------------------------------------------------------------------------
// Structural shape for clone detection (normalized)
// ---------------------------------------------------------------------------

fn shape_expr(expr: &Expr, out: &mut String) {
    match expr {
        Expr::Constant(_) => out.push_str("LIT|"),
        Expr::Name(_) => out.push_str("NAME|"),
        Expr::BoolOp(b) => {
            out.push_str("BOOLOP|");
            for v in &b.values {
                shape_expr(v, out);
            }
        }
        Expr::BinOp(b) => {
            out.push_str("BINOP|");
            shape_expr(&b.left, out);
            shape_expr(&b.right, out);
        }
        Expr::UnaryOp(u) => {
            out.push_str("UNOP|");
            shape_expr(&u.operand, out);
        }
        Expr::Lambda(l) => {
            out.push_str("LAMBDA|");
            shape_expr(&l.body, out);
        }
        Expr::IfExp(i) => {
            out.push_str("IFEXP|");
            shape_expr(&i.test, out);
            shape_expr(&i.body, out);
            shape_expr(&i.orelse, out);
        }
        Expr::Dict(d) => {
            out.push_str("DICT|");
            for (maybe_k, v) in d.keys.iter().zip(&d.values) {
                if let Some(k) = maybe_k {
                    shape_expr(k, out);
                }
                shape_expr(v, out);
            }
        }
        Expr::Set(s) => {
            out.push_str("SET|");
            for e in &s.elts {
                shape_expr(e, out);
            }
        }
        Expr::List(l) => {
            out.push_str("LIST|");
            for e in &l.elts {
                shape_expr(e, out);
            }
        }
        Expr::Tuple(t) => {
            out.push_str("TUPLE|");
            for e in &t.elts {
                shape_expr(e, out);
            }
        }
        Expr::ListComp(l) => {
            out.push_str("LISTCOMP|");
            shape_expr(&l.elt, out);
            for g in &l.generators {
                shape_expr(&g.target, out);
                shape_expr(&g.iter, out);
                for iff in &g.ifs {
                    shape_expr(iff, out);
                }
            }
        }
        Expr::SetComp(s) => {
            out.push_str("SETCOMP|");
            shape_expr(&s.elt, out);
            for g in &s.generators {
                shape_expr(&g.target, out);
                shape_expr(&g.iter, out);
                for iff in &g.ifs {
                    shape_expr(iff, out);
                }
            }
        }
        Expr::DictComp(d) => {
            out.push_str("DICTCOMP|");
            shape_expr(&d.key, out);
            shape_expr(&d.value, out);
            for g in &d.generators {
                shape_expr(&g.target, out);
                shape_expr(&g.iter, out);
                for iff in &g.ifs {
                    shape_expr(iff, out);
                }
            }
        }
        Expr::GeneratorExp(g) => {
            out.push_str("GENEXP|");
            shape_expr(&g.elt, out);
            for gen in &g.generators {
                shape_expr(&gen.target, out);
                shape_expr(&gen.iter, out);
                for iff in &gen.ifs {
                    shape_expr(iff, out);
                }
            }
        }
        Expr::Await(a) => {
            out.push_str("AWAIT|");
            shape_expr(&a.value, out);
        }
        Expr::Yield(y) => {
            out.push_str("YIELD|");
            if let Some(v) = &y.value {
                shape_expr(v, out);
            }
        }
        Expr::YieldFrom(y) => {
            out.push_str("YIELDFROM|");
            shape_expr(&y.value, out);
        }
        Expr::Compare(c) => {
            out.push_str("CMP|");
            shape_expr(&c.left, out);
            for comp in &c.comparators {
                shape_expr(comp, out);
            }
        }
        Expr::Call(c) => {
            out.push_str("CALL|");
            shape_expr(&c.func, out);
            for a in &c.args {
                shape_expr(a, out);
            }
            for kw in &c.keywords {
                shape_expr(&kw.value, out);
            }
        }
        Expr::FormattedValue(f) => {
            out.push_str("FVAL|");
            shape_expr(&f.value, out);
        }
        Expr::JoinedStr(j) => {
            out.push_str("FSTR|");
            for v in &j.values {
                shape_expr(v, out);
            }
        }
        Expr::Attribute(a) => {
            out.push_str("ATTR|");
            shape_expr(&a.value, out);
        }
        Expr::Subscript(s) => {
            out.push_str("SUB|");
            shape_expr(&s.value, out);
            shape_expr(&s.slice, out);
        }
        Expr::Starred(s) => {
            out.push_str("STAR|");
            shape_expr(&s.value, out);
        }
        Expr::NamedExpr(n) => {
            out.push_str("NAMED|");
            shape_expr(&n.target, out);
            shape_expr(&n.value, out);
        }
        _ => out.push_str("EXPR|"),
    }
}

fn shape_stmt(stmt: &Stmt, out: &mut String) {
    match stmt {
        Stmt::FunctionDef(f) => {
            out.push_str("DEF|");
            for st in &f.body {
                shape_stmt(st, out);
            }
        }
        Stmt::AsyncFunctionDef(f) => {
            out.push_str("ASYNCDEF|");
            for st in &f.body {
                shape_stmt(st, out);
            }
        }
        Stmt::ClassDef(c) => {
            out.push_str("CLASS|");
            for st in &c.body {
                shape_stmt(st, out);
            }
        }
        Stmt::Return(r) => {
            out.push_str("RET|");
            if let Some(v) = &r.value {
                shape_expr(v, out);
            }
        }
        Stmt::Delete(d) => {
            out.push_str("DEL|");
            for t in &d.targets {
                shape_expr(t, out);
            }
        }
        Stmt::Assign(a) => {
            out.push_str("ASSIGN|");
            for t in &a.targets {
                shape_expr(t, out);
            }
            shape_expr(&a.value, out);
        }
        Stmt::AugAssign(a) => {
            out.push_str("AUGASSIGN|");
            shape_expr(&a.target, out);
            shape_expr(&a.value, out);
        }
        Stmt::AnnAssign(a) => {
            out.push_str("ANNASSIGN|");
            shape_expr(&a.target, out);
            if let Some(v) = &a.value {
                shape_expr(v, out);
            }
        }
        Stmt::For(f) => {
            out.push_str("FOR|");
            shape_expr(&f.target, out);
            shape_expr(&f.iter, out);
            for st in &f.body {
                shape_stmt(st, out);
            }
            for st in &f.orelse {
                shape_stmt(st, out);
            }
        }
        Stmt::AsyncFor(f) => {
            out.push_str("AFOR|");
            shape_expr(&f.target, out);
            shape_expr(&f.iter, out);
            for st in &f.body {
                shape_stmt(st, out);
            }
            for st in &f.orelse {
                shape_stmt(st, out);
            }
        }
        Stmt::While(w) => {
            out.push_str("WHILE|");
            shape_expr(&w.test, out);
            for st in &w.body {
                shape_stmt(st, out);
            }
            for st in &w.orelse {
                shape_stmt(st, out);
            }
        }
        Stmt::If(i) => {
            out.push_str("IF|");
            shape_expr(&i.test, out);
            for st in &i.body {
                shape_stmt(st, out);
            }
            for st in &i.orelse {
                shape_stmt(st, out);
            }
        }
        Stmt::With(w) => {
            out.push_str("WITH|");
            for st in &w.body {
                shape_stmt(st, out);
            }
        }
        Stmt::AsyncWith(w) => {
            out.push_str("AWITH|");
            for st in &w.body {
                shape_stmt(st, out);
            }
        }
        Stmt::Match(m) => {
            out.push_str("MATCH|");
            shape_expr(&m.subject, out);
            for case in &m.cases {
                for st in &case.body {
                    shape_stmt(st, out);
                }
            }
        }
        Stmt::Raise(r) => {
            out.push_str("RAISE|");
            if let Some(e) = &r.exc {
                shape_expr(e, out);
            }
        }
        Stmt::Try(t) => {
            out.push_str("TRY|");
            for st in &t.body {
                shape_stmt(st, out);
            }
            for st in &t.orelse {
                shape_stmt(st, out);
            }
            for st in &t.finalbody {
                shape_stmt(st, out);
            }
        }
        Stmt::TryStar(t) => {
            out.push_str("TRYSTAR|");
            for st in &t.body {
                shape_stmt(st, out);
            }
            for st in &t.orelse {
                shape_stmt(st, out);
            }
            for st in &t.finalbody {
                shape_stmt(st, out);
            }
        }
        Stmt::Assert(a) => {
            out.push_str("ASSERT|");
            shape_expr(&a.test, out);
            if let Some(msg) = &a.msg {
                shape_expr(msg, out);
            }
        }
        Stmt::Import(_) => out.push_str("IMPORT|"),
        Stmt::ImportFrom(_) => out.push_str("IMPORTFROM|"),
        Stmt::Global(_) => out.push_str("GLOBAL|"),
        Stmt::Nonlocal(_) => out.push_str("NONLOCAL|"),
        Stmt::Pass(_) => out.push_str("PASS|"),
        Stmt::Break(_) => out.push_str("BREAK|"),
        Stmt::Continue(_) => out.push_str("CONTINUE|"),
        Stmt::Expr(e) => {
            out.push_str("EXPRSTMT|");
            shape_expr(&e.value, out);
        }
        Stmt::TypeAlias(_) => out.push_str("TYPEALIAS|"),
    }
}

pub(super) fn python_body_shape_hash(body: &[Stmt]) -> u64 {
    let mut s = String::new();
    for st in body {
        shape_stmt(st, &mut s);
    }
    clones::hash_shape(&s)
}

fn assign_target_hint(expr: &Expr) -> String {
    match expr {
        Expr::Name(n) => n.id.to_string(),
        Expr::Attribute(a) => {
            format!("{}.{}", assign_target_hint(&a.value), a.attr)
        }
        Expr::Subscript(sub) => {
            format!("{}[]", assign_target_hint(&sub.value))
        }
        Expr::Tuple(t) => t
            .elts
            .iter()
            .map(assign_target_hint)
            .collect::<Vec<_>>()
            .join(","),
        _ => "?".into(),
    }
}

pub(super) fn collect_py_security(
    body: &[Stmt],
    filepath: &str,
    source: &str,
    out: &mut Vec<SecurityFinding>,
) {
    for stmt in body {
        security_stmt(stmt, filepath, source, out);
    }
}

fn security_stmt(stmt: &Stmt, filepath: &str, source: &str, out: &mut Vec<SecurityFinding>) {
    match stmt {
        Stmt::Assign(a) => {
            let hint = a
                .targets
                .first()
                .map(assign_target_hint)
                .unwrap_or_default();
            if let Expr::Constant(c) = a.value.as_ref() {
                if let Constant::Str(val) = &c.value {
                    let line = line_at(source, c.range().start());
                    if let Some(f) = secrets::audit_string_literal(val, filepath, line, &hint) {
                        out.push(f);
                    }
                }
            }
        }
        Stmt::AnnAssign(a) => {
            let hint = assign_target_hint(&a.target);
            if let Some(v) = &a.value {
                if let Expr::Constant(c) = v.as_ref() {
                    if let Constant::Str(val) = &c.value {
                        let line = line_at(source, c.range().start());
                        if let Some(f) = secrets::audit_string_literal(val, filepath, line, &hint) {
                            out.push(f);
                        }
                    }
                }
            }
        }
        _ => {}
    }
    for_each_child_stmt(stmt, &mut |child| security_stmt(child, filepath, source, out));
    for_each_handler(stmt, &mut |h| {
        let ExceptHandler::ExceptHandler(eh) = h;
        for st in &eh.body {
            security_stmt(st, filepath, source, out);
        }
    });
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
