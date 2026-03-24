//! `syn` walks for cyclomatic complexity, nesting, unsafe blocks, unwrap/expect.

use std::collections::HashMap;

use proc_macro2::Span;
use syn::visit::Visit;
use syn::{
    BinOp, Block, Expr, ExprBinary, ExprForLoop, ExprIf, ExprLoop, ExprMatch, ExprTry, ExprUnsafe,
    ExprWhile,
};

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
