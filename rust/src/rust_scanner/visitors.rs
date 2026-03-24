//! `syn` walks for cyclomatic complexity, nesting, unsafe blocks, unwrap/expect.

use std::collections::HashMap;

use proc_macro2::Span;
use syn::visit::Visit;
use syn::{
    BinOp, Block, Expr, ExprBinary, ExprForLoop, ExprIf, ExprLoop, ExprMatch, ExprTry, ExprUnsafe,
    ExprWhile, Lit, Signature, Stmt,
};

use crate::clones;
use crate::types::SecurityFinding;

pub(crate) fn span_line(span: Span) -> usize {
    span.start().line
}

pub(crate) fn span_end_line(span: Span) -> usize {
    span.end().line
}

/// Base 1 + decision points (aligned with TS/Python style).
pub(crate) fn complexity_block(block: &Block) -> usize {
    let mut v = ComplexityWalk { cc: 1 };
    v.visit_block(block);
    v.cc
}

struct ComplexityWalk {
    cc: usize,
}

impl<'ast> Visit<'ast> for ComplexityWalk {
    fn visit_expr_if(&mut self, node: &'ast ExprIf) {
        self.cc += 1;
        syn::visit::visit_expr_if(self, node);
    }

    fn visit_expr_while(&mut self, node: &'ast ExprWhile) {
        self.cc += 1;
        syn::visit::visit_expr_while(self, node);
    }

    fn visit_expr_for_loop(&mut self, node: &'ast ExprForLoop) {
        self.cc += 1;
        syn::visit::visit_expr_for_loop(self, node);
    }

    fn visit_expr_loop(&mut self, node: &'ast ExprLoop) {
        self.cc += 1;
        syn::visit::visit_expr_loop(self, node);
    }

    fn visit_expr_match(&mut self, node: &'ast ExprMatch) {
        self.cc += node.arms.len().max(1);
        syn::visit::visit_expr_match(self, node);
    }

    fn visit_expr_binary(&mut self, node: &'ast ExprBinary) {
        if matches!(node.op, BinOp::And(_) | BinOp::Or(_)) {
            self.cc += 1;
        }
        syn::visit::visit_expr_binary(self, node);
    }

    fn visit_expr_try(&mut self, node: &'ast ExprTry) {
        self.cc += 1;
        syn::visit::visit_expr_try(self, node);
    }
}

pub(crate) fn nesting_block(block: &Block) -> usize {
    let mut v = NestingWalk { depth: 0, max_d: 0 };
    v.visit_block(block);
    v.max_d as usize
}

struct NestingWalk {
    depth: u32,
    max_d: u32,
}

impl NestingWalk {
    fn enter(&mut self) {
        self.depth += 1;
        self.max_d = self.max_d.max(self.depth);
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }
}

impl<'ast> Visit<'ast> for NestingWalk {
    fn visit_expr_if(&mut self, node: &'ast ExprIf) {
        self.enter();
        syn::visit::visit_expr_if(self, node);
        self.leave();
    }

    fn visit_expr_while(&mut self, node: &'ast ExprWhile) {
        self.enter();
        syn::visit::visit_expr_while(self, node);
        self.leave();
    }

    fn visit_expr_for_loop(&mut self, node: &'ast ExprForLoop) {
        self.enter();
        syn::visit::visit_expr_for_loop(self, node);
        self.leave();
    }

    fn visit_expr_loop(&mut self, node: &'ast ExprLoop) {
        self.enter();
        syn::visit::visit_expr_loop(self, node);
        self.leave();
    }

    fn visit_expr_match(&mut self, node: &'ast ExprMatch) {
        self.enter();
        syn::visit::visit_expr_match(self, node);
        self.leave();
    }
}

pub(crate) fn count_unsafe_blocks(block: &Block) -> usize {
    let mut v = UnsafeWalk { count: 0 };
    v.visit_block(block);
    v.count
}

struct UnsafeWalk {
    count: usize,
}

impl<'ast> Visit<'ast> for UnsafeWalk {
    fn visit_expr_unsafe(&mut self, node: &'ast ExprUnsafe) {
        self.count += 1;
        syn::visit::visit_expr_unsafe(self, node);
    }
}

pub(crate) fn count_unwrap_expect_in_block(block: &Block) -> usize {
    let mut v = UnwrapWalk { count: 0 };
    v.visit_block(block);
    v.count
}

struct UnwrapWalk {
    count: usize,
}

impl<'ast> Visit<'ast> for UnwrapWalk {
    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        let m = node.method.to_string();
        if m == "unwrap" || m == "expect" {
            self.count += 1;
        }
        syn::visit::visit_expr_method_call(self, node);
    }
}

pub(crate) fn count_unwrap_expect_expr(expr: &Expr) -> usize {
    let mut v = UnwrapWalk { count: 0 };
    v.visit_expr(expr);
    v.count
}

struct UnsafeExprWalk {
    count: usize,
}

impl<'ast> Visit<'ast> for UnsafeExprWalk {
    fn visit_expr_unsafe(&mut self, node: &'ast ExprUnsafe) {
        self.count += 1;
        syn::visit::visit_expr_unsafe(self, node);
    }
}

pub(crate) fn count_unsafe_expr(expr: &Expr) -> usize {
    let mut v = UnsafeExprWalk { count: 0 };
    v.visit_expr(expr);
    v.count
}

fn path_to_string(path: &syn::Path) -> String {
    path.segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}

/// Collect `#[allow(...)]` lint names from attributes.
pub(crate) fn collect_allow_lints(attrs: &[syn::Attribute], out: &mut HashMap<String, usize>) {
    for attr in attrs {
        if !attr.path().is_ident("allow") {
            continue;
        }
        let _ = attr.parse_nested_meta(|meta| {
            let name = path_to_string(&meta.path);
            if !name.is_empty() {
                *out.entry(name).or_insert(0) += 1;
            }
            Ok(())
        });
    }
}

/// Derive macro paths from `#[derive(Debug, Clone)]`.
pub(crate) fn collect_derives(attrs: &[syn::Attribute], out: &mut HashMap<String, usize>) {
    for attr in attrs {
        if !attr.path().is_ident("derive") {
            continue;
        }
        if let Ok(paths) = attr.parse_args_with(
            syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated,
        ) {
            for p in paths {
                if let Some(s) = p.segments.last() {
                    *out.entry(s.ident.to_string()).or_insert(0) += 1;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Cognitive complexity (nesting-weighted)
// ---------------------------------------------------------------------------

pub(crate) fn cognitive_block(block: &Block) -> usize {
    let mut v = CognitiveWalk { score: 0, nest: 0 };
    v.visit_block(block);
    v.score
}

struct CognitiveWalk {
    score: usize,
    nest: usize,
}

impl CognitiveWalk {
    fn with_nest<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        self.nest += 1;
        let r = f(self);
        self.nest -= 1;
        r
    }
}

impl<'ast> Visit<'ast> for CognitiveWalk {
    fn visit_expr_if(&mut self, node: &'ast ExprIf) {
        self.score += 1 + self.nest;
        self.visit_expr(&node.cond);
        self.with_nest(|s| s.visit_block(&node.then_branch));
        if let Some((_, el)) = &node.else_branch {
            self.with_nest(|s| s.visit_expr(el));
        }
    }

    fn visit_expr_while(&mut self, node: &'ast ExprWhile) {
        self.score += 1 + self.nest;
        self.visit_expr(&node.cond);
        self.with_nest(|s| s.visit_block(&node.body));
    }

    fn visit_expr_for_loop(&mut self, node: &'ast ExprForLoop) {
        self.score += 1 + self.nest;
        self.visit_pat(&node.pat);
        self.visit_expr(&node.expr);
        self.with_nest(|s| s.visit_block(&node.body));
    }

    fn visit_expr_loop(&mut self, node: &'ast ExprLoop) {
        self.score += 1 + self.nest;
        self.with_nest(|s| s.visit_block(&node.body));
    }

    fn visit_expr_match(&mut self, node: &'ast ExprMatch) {
        self.score += 1 + self.nest;
        self.visit_expr(&node.expr);
        self.with_nest(|s| {
            for arm in &node.arms {
                if let Some((_, guard)) = &arm.guard {
                    s.visit_expr(guard);
                }
                s.visit_expr(&arm.body);
            }
        });
    }

    fn visit_expr_binary(&mut self, node: &'ast ExprBinary) {
        if matches!(node.op, BinOp::And(_) | BinOp::Or(_)) {
            self.score += 1 + self.nest;
        }
        syn::visit::visit_expr_binary(self, node);
    }

    fn visit_expr_try(&mut self, node: &'ast ExprTry) {
        self.score += 1 + self.nest;
        self.with_nest(|s| s.visit_expr(&node.expr));
    }
}

pub(crate) fn count_rust_params(sig: &Signature, is_method: bool) -> usize {
    let n = sig.inputs.len();
    if is_method
        && !sig.inputs.is_empty()
        && matches!(sig.inputs.first(), Some(syn::FnArg::Receiver(_)))
    {
        return n.saturating_sub(1);
    }
    n
}

// ---------------------------------------------------------------------------
// Structural shape hash (clone detection)
// ---------------------------------------------------------------------------

fn rust_append_expr_shape(expr: &Expr, out: &mut String) {
    match expr {
        Expr::Array(a) => {
            out.push_str("ARR|");
            for e in &a.elems {
                rust_append_expr_shape(e, out);
            }
        }
        Expr::Assign(a) => {
            out.push_str("ASSIGN|");
            rust_append_expr_shape(&a.left, out);
            rust_append_expr_shape(&a.right, out);
        }
        Expr::Async(_) => out.push_str("ASYNC|"),
        Expr::Await(a) => {
            out.push_str("AWAIT|");
            rust_append_expr_shape(&a.base, out);
        }
        Expr::Binary(b) => {
            out.push_str("BIN|");
            rust_append_expr_shape(&b.left, out);
            rust_append_expr_shape(&b.right, out);
        }
        Expr::Block(b) => {
            out.push_str("BLK|");
            rust_append_block_shape(&b.block, out);
        }
        Expr::Break(_) => out.push_str("BREAK|"),
        Expr::Call(c) => {
            out.push_str("CALL|");
            rust_append_expr_shape(&c.func, out);
            for a in &c.args {
                rust_append_expr_shape(a, out);
            }
        }
        Expr::Cast(c) => {
            out.push_str("CAST|");
            rust_append_expr_shape(&c.expr, out);
        }
        Expr::Closure(c) => {
            out.push_str("CLOSURE|");
            rust_append_expr_shape(&c.body, out);
        }
        Expr::Const(_) => out.push_str("CONST|"),
        Expr::Continue(_) => out.push_str("CONT|"),
        Expr::Field(f) => {
            out.push_str("FIELD|");
            rust_append_expr_shape(&f.base, out);
        }
        Expr::ForLoop(f) => {
            out.push_str("FOR|");
            rust_append_expr_shape(&f.expr, out);
            rust_append_block_shape(&f.body, out);
        }
        Expr::Group(g) => rust_append_expr_shape(&g.expr, out),
        Expr::If(i) => {
            out.push_str("IF|");
            rust_append_expr_shape(&i.cond, out);
            rust_append_block_shape(&i.then_branch, out);
            if let Some((_, e)) = &i.else_branch {
                rust_append_expr_shape(e, out);
            }
        }
        Expr::Index(i) => {
            out.push_str("IDX|");
            rust_append_expr_shape(&i.expr, out);
            rust_append_expr_shape(&i.index, out);
        }
        Expr::Let(l) => {
            out.push_str("LET|");
            rust_append_expr_shape(&l.expr, out);
        }
        Expr::Lit(_) => out.push_str("LIT|"),
        Expr::Loop(l) => {
            out.push_str("LOOP|");
            rust_append_block_shape(&l.body, out);
        }
        Expr::Macro(m) => {
            out.push_str("MACRO|");
            let _ = m;
        }
        Expr::Match(m) => {
            out.push_str("MATCH|");
            rust_append_expr_shape(&m.expr, out);
            for arm in &m.arms {
                rust_append_expr_shape(&arm.body, out);
            }
        }
        Expr::MethodCall(m) => {
            out.push_str("METHOD|");
            rust_append_expr_shape(&m.receiver, out);
            for a in &m.args {
                rust_append_expr_shape(a, out);
            }
        }
        Expr::Paren(p) => rust_append_expr_shape(&p.expr, out),
        Expr::Path(_) => out.push_str("PATH|"),
        Expr::Range(_) => out.push_str("RANGE|"),
        Expr::Reference(r) => {
            out.push_str("REF|");
            rust_append_expr_shape(&r.expr, out);
        }
        Expr::Repeat(r) => {
            out.push_str("REP|");
            rust_append_expr_shape(&r.expr, out);
            rust_append_expr_shape(&r.len, out);
        }
        Expr::Return(r) => {
            out.push_str("RET|");
            if let Some(e) = &r.expr {
                rust_append_expr_shape(e, out);
            }
        }
        Expr::Struct(s) => {
            out.push_str("STRUCT|");
            for f in &s.fields {
                rust_append_expr_shape(&f.expr, out);
            }
        }
        Expr::Try(t) => {
            out.push_str("TRY|");
            rust_append_expr_shape(&t.expr, out);
        }
        Expr::TryBlock(t) => {
            out.push_str("TRYBLK|");
            rust_append_block_shape(&t.block, out);
        }
        Expr::Tuple(t) => {
            out.push_str("TUP|");
            for e in &t.elems {
                rust_append_expr_shape(e, out);
            }
        }
        Expr::Unary(u) => {
            out.push_str("UNARY|");
            rust_append_expr_shape(&u.expr, out);
        }
        Expr::Unsafe(u) => {
            out.push_str("UNSAFE|");
            rust_append_block_shape(&u.block, out);
        }
        Expr::While(w) => {
            out.push_str("WHILE|");
            rust_append_expr_shape(&w.cond, out);
            rust_append_block_shape(&w.body, out);
        }
        Expr::Yield(y) => {
            out.push_str("YIELD|");
            if let Some(e) = &y.expr {
                rust_append_expr_shape(e, out);
            }
        }
        _ => out.push_str("EXPR|"),
    }
}

fn rust_append_stmt_shape(stmt: &Stmt, out: &mut String) {
    match stmt {
        Stmt::Local(l) => {
            out.push_str("LOCAL|");
            if let Some(init) = &l.init {
                rust_append_expr_shape(&init.expr, out);
            }
        }
        Stmt::Item(_) => out.push_str("ITEM|"),
        Stmt::Expr(e, _) => {
            out.push_str("EXPRSTMT|");
            rust_append_expr_shape(e, out);
        }
        Stmt::Macro(_) => out.push_str("MACRO|"),
    }
}

fn rust_append_block_shape(block: &Block, out: &mut String) {
    for stmt in &block.stmts {
        rust_append_stmt_shape(stmt, out);
    }
}

pub(crate) fn rust_block_shape_hash(block: &Block) -> u64 {
    let mut s = String::new();
    rust_append_block_shape(block, &mut s);
    clones::hash_shape(&s)
}

// ---------------------------------------------------------------------------
// Security: string literals in expressions
// ---------------------------------------------------------------------------

struct RustSecWalk<'a> {
    file: &'a str,
    out: &'a mut Vec<SecurityFinding>,
}

impl<'ast> Visit<'ast> for RustSecWalk<'_> {
    fn visit_lit(&mut self, lit: &'ast Lit) {
        if let Lit::Str(s) = lit {
            let line = span_line(s.span());
            if let Some(f) =
                crate::secrets::audit_string_literal(&s.value(), self.file, line, "literal")
            {
                self.out.push(f);
            }
        }
    }
}

pub(crate) fn collect_rust_security_block(block: &Block, file: &str, out: &mut Vec<SecurityFinding>) {
    let mut w = RustSecWalk { file, out };
    w.visit_block(block);
}

pub(crate) fn collect_rust_security_expr(expr: &Expr, file: &str, out: &mut Vec<SecurityFinding>) {
    let mut w = RustSecWalk { file, out };
    w.visit_expr(expr);
}
