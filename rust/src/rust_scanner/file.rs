//! Per-file Rust analysis with `syn`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use syn::spanned::Spanned;
use syn::{Attribute, Block, Expr, ImplItem, Item, Signature, Type, UseTree, Visibility};

use crate::types::{RsFileData, RsFuncInfo, RsImportInfo, RsStructInfo, RsTraitInfo};

use super::visitors::{
    collect_allow_lints, collect_derives, complexity_block, count_unsafe_blocks, count_unsafe_expr,
    count_unwrap_expect_expr, count_unwrap_expect_in_block, nesting_block, span_end_line,
    span_line,
};

pub(crate) fn display_rel(abs: &Path, scan_root: &Path) -> String {
    abs.strip_prefix(scan_root)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| abs.display().to_string())
        .replace('\\', "/")
}

fn should_skip_rs_dir(name: &str) -> bool {
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

pub(crate) fn collect_rs_files(scan_root: &Path, exclude: &[String]) -> Vec<PathBuf> {
    let mut result = Vec::new();
    let mut stack = vec![scan_root.to_path_buf()];
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
                if should_skip_rs_dir(n.as_ref()) {
                    continue;
                }
                if !exclude.is_empty() && matches_exclude(&full, scan_root, exclude) {
                    continue;
                }
                stack.push(full);
            } else if n.ends_with(".rs")
                && (exclude.is_empty() || !matches_exclude(&full, scan_root, exclude))
            {
                result.push(full);
            }
        }
    }
    result.sort();
    result
}

/// Module id from repo-relative path: `src/foo/bar.rs` -> `foo/bar`, `foo/mod.rs` -> `foo`.
pub(crate) fn rust_file_to_module(rel: &str) -> String {
    let p = rel.replace('\\', "/");
    let p = p.strip_suffix(".rs").unwrap_or(&p);
    if let Some(stripped) = p.strip_suffix("/mod") {
        stripped.to_string()
    } else {
        p.to_string()
    }
}

fn vis_string(v: &Visibility) -> String {
    match v {
        Visibility::Public(_) => "pub".to_string(),
        Visibility::Restricted(r) => format!("pub({})", quote_path(&r.path)),
        Visibility::Inherited => "".to_string(),
    }
}

fn quote_path(path: &syn::Path) -> String {
    path.segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}

fn item_attrs(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(x) => &x.attrs,
        Item::Enum(x) => &x.attrs,
        Item::ExternCrate(x) => &x.attrs,
        Item::Fn(x) => &x.attrs,
        Item::ForeignMod(x) => &x.attrs,
        Item::Impl(x) => &x.attrs,
        Item::Macro(x) => &x.attrs,
        Item::Mod(x) => &x.attrs,
        Item::Static(x) => &x.attrs,
        Item::Struct(x) => &x.attrs,
        Item::Trait(x) => &x.attrs,
        Item::TraitAlias(x) => &x.attrs,
        Item::Type(x) => &x.attrs,
        Item::Union(x) => &x.attrs,
        Item::Use(x) => &x.attrs,
        _ => &[],
    }
}

fn current_module_parts(current_mod: &str) -> Vec<&str> {
    current_mod.split('/').filter(|s| !s.is_empty()).collect()
}

fn join_mod(parts: &[String]) -> String {
    parts.join("/")
}

/// Resolve `crate::a::b` / `super::x` / `self::y` to slash module id.
fn resolve_internal_module_path(current_mod: &str, segments: &[String]) -> String {
    if segments.is_empty() {
        return current_mod.to_string();
    }
    match segments[0].as_str() {
        "crate" => segments[1..].join("/"),
        "self" => {
            let mut base: Vec<String> = current_module_parts(current_mod)
                .iter()
                .map(|s| (*s).to_string())
                .collect();
            base.extend(segments[1..].iter().cloned());
            join_mod(&base)
        }
        "super" => {
            let mut parts_vec: Vec<String> = current_module_parts(current_mod)
                .iter()
                .map(|s| (*s).to_string())
                .collect();
            let mut i = 0;
            while i < segments.len() && segments[i] == "super" {
                parts_vec.pop();
                i += 1;
            }
            parts_vec.extend(segments[i..].iter().cloned());
            join_mod(&parts_vec)
        }
        _ => segments.join("/"),
    }
}

fn record_use_tree(
    current_mod: &str,
    tree: &UseTree,
    prefix: &mut Vec<String>,
    line: usize,
    imports: &mut Vec<RsImportInfo>,
) {
    match tree {
        UseTree::Path(p) => {
            prefix.push(p.ident.to_string());
            record_use_tree(current_mod, &p.tree, prefix, line, imports);
            prefix.pop();
        }
        UseTree::Name(n) => {
            let mut segs = prefix.clone();
            segs.push(n.ident.to_string());
            let source = segs.join("::");
            let (is_internal, resolved) = if let Some(first) = prefix.first().map(String::as_str) {
                match first {
                    "crate" | "super" | "self" => {
                        (true, resolve_internal_module_path(current_mod, &segs))
                    }
                    _ => (false, prefix.join("/")),
                }
            } else {
                (false, String::new())
            };
            imports.push(RsImportInfo {
                source: source.clone(),
                specifiers: vec![n.ident.to_string()],
                is_internal,
                resolved_path: resolved,
                line,
            });
        }
        UseTree::Rename(r) => {
            let mut segs = prefix.clone();
            segs.push(r.ident.to_string());
            let source = segs.join("::");
            let (is_internal, resolved) = if let Some(first) = prefix.first().map(String::as_str) {
                match first {
                    "crate" | "super" | "self" => {
                        (true, resolve_internal_module_path(current_mod, &segs))
                    }
                    _ => (false, prefix.join("/")),
                }
            } else {
                (false, String::new())
            };
            imports.push(RsImportInfo {
                source,
                specifiers: vec![r.rename.to_string()],
                is_internal,
                resolved_path: resolved,
                line,
            });
        }
        UseTree::Glob(_) => {
            let source = prefix.join("::");
            let (is_internal, resolved) = if let Some(first) = prefix.first().map(String::as_str) {
                match first {
                    "crate" | "super" | "self" => {
                        let full: Vec<String> = prefix.to_vec();
                        (true, resolve_internal_module_path(current_mod, &full))
                    }
                    _ => (false, prefix.join("/")),
                }
            } else {
                (false, String::new())
            };
            imports.push(RsImportInfo {
                source,
                specifiers: vec!["*".to_string()],
                is_internal,
                resolved_path: resolved,
                line,
            });
        }
        UseTree::Group(g) => {
            for t in &g.items {
                record_use_tree(current_mod, t, prefix, line, imports);
            }
        }
    }
}

fn process_use_item(
    current_mod: &str,
    item: &syn::ItemUse,
    imports: &mut Vec<RsImportInfo>,
    exports: &mut Vec<String>,
) {
    let line = span_line(item.span());
    if matches!(item.vis, Visibility::Public(_)) {
        let mut prefix = Vec::new();
        collect_pub_use_names(&item.tree, &mut prefix, exports);
    }
    let mut prefix = Vec::new();
    record_use_tree(current_mod, &item.tree, &mut prefix, line, imports);
}

fn collect_pub_use_names(tree: &UseTree, prefix: &mut Vec<String>, exports: &mut Vec<String>) {
    match tree {
        UseTree::Path(p) => {
            prefix.push(p.ident.to_string());
            collect_pub_use_names(&p.tree, prefix, exports);
            prefix.pop();
        }
        UseTree::Name(n) => {
            exports.push(n.ident.to_string());
        }
        UseTree::Rename(r) => {
            exports.push(r.rename.to_string());
        }
        UseTree::Glob(_) => {
            exports.push("*".to_string());
        }
        UseTree::Group(g) => {
            for t in &g.items {
                collect_pub_use_names(t, prefix, exports);
            }
        }
    }
}

struct RsFnSpec<'a> {
    sig: &'a Signature,
    vis: &'a Visibility,
    block: Option<&'a Block>,
    file: &'a str,
    parent_type: Option<String>,
    is_method: bool,
}

fn push_fn(out: &mut Vec<RsFuncInfo>, spec: RsFnSpec<'_>) {
    let line = span_line(spec.sig.fn_token.span);
    let end_line = spec
        .block
        .map(|b| span_end_line(b.brace_token.span.close()))
        .unwrap_or_else(|| span_end_line(spec.sig.ident.span()));
    let line_count = end_line.saturating_sub(line) + 1;
    let (cc, nest) = if let Some(b) = spec.block {
        (complexity_block(b), nesting_block(b))
    } else {
        (1usize, 0usize)
    };
    let name = spec.sig.ident.to_string();
    let qualname = match &spec.parent_type {
        Some(p) => format!("{p}::{name}"),
        None => name.clone(),
    };
    let vis = vis_string(spec.vis);
    let is_unsafe = spec.sig.unsafety.is_some();
    let is_async = spec.sig.asyncness.is_some();
    out.push(RsFuncInfo {
        name,
        qualname,
        file: spec.file.to_string(),
        line,
        end_line,
        line_count,
        complexity: cc,
        nesting: nest,
        is_method: spec.is_method,
        is_unsafe,
        is_async,
        visibility: vis,
        parent_type: spec.parent_type,
    });
}

struct RsItemCtx<'a> {
    current_mod: &'a str,
    rel_file: &'a str,
    functions: &'a mut Vec<RsFuncInfo>,
    structs: &'a mut Vec<RsStructInfo>,
    traits: &'a mut Vec<RsTraitInfo>,
    imports: &'a mut Vec<RsImportInfo>,
    exports: &'a mut Vec<String>,
    allow_lints: &'a mut HashMap<String, usize>,
    derive_hits: &'a mut HashMap<String, usize>,
    unsafe_blocks: &'a mut usize,
    unwrap_expect: &'a mut usize,
}

fn process_fn_item(ctx: &mut RsItemCtx<'_>, f: &syn::ItemFn) {
    if matches!(f.vis, Visibility::Public(_)) {
        ctx.exports.push(f.sig.ident.to_string());
    }
    let body = f.block.as_ref();
    *ctx.unsafe_blocks += count_unsafe_blocks(body);
    *ctx.unwrap_expect += count_unwrap_expect_in_block(body);
    push_fn(
        ctx.functions,
        RsFnSpec {
            sig: &f.sig,
            vis: &f.vis,
            block: Some(body),
            file: ctx.rel_file,
            parent_type: None,
            is_method: false,
        },
    );
}

fn process_struct_item(ctx: &mut RsItemCtx<'_>, s: &syn::ItemStruct) {
    if matches!(s.vis, Visibility::Public(_)) {
        ctx.exports.push(s.ident.to_string());
    }
    collect_derives(&s.attrs, ctx.derive_hits);
    collect_allow_lints(&s.attrs, ctx.allow_lints);
    let line = span_line(s.struct_token.span);
    let end = span_end_line(s.fields.span());
    let line_count = end.saturating_sub(line) + 1;
    let fields_count = match &s.fields {
        syn::Fields::Named(f) => f.named.len(),
        syn::Fields::Unnamed(f) => f.unnamed.len(),
        syn::Fields::Unit => 0,
    };
    let derives: Vec<String> = {
        let mut m = HashMap::new();
        collect_derives(&s.attrs, &mut m);
        m.keys().cloned().collect()
    };
    ctx.structs.push(RsStructInfo {
        name: s.ident.to_string(),
        kind: "struct".to_string(),
        file: ctx.rel_file.to_string(),
        line,
        line_count,
        fields_count,
        methods_count: 0,
        derives,
        has_generics: !s.generics.params.is_empty(),
        visibility: vis_string(&s.vis),
    });
}

fn process_enum_item(ctx: &mut RsItemCtx<'_>, e: &syn::ItemEnum) {
    if matches!(e.vis, Visibility::Public(_)) {
        ctx.exports.push(e.ident.to_string());
    }
    collect_derives(&e.attrs, ctx.derive_hits);
    collect_allow_lints(&e.attrs, ctx.allow_lints);
    let line = span_line(e.enum_token.span);
    let end = span_end_line(e.variants.span());
    let line_count = end.saturating_sub(line) + 1;
    ctx.structs.push(RsStructInfo {
        name: e.ident.to_string(),
        kind: "enum".to_string(),
        file: ctx.rel_file.to_string(),
        line,
        line_count,
        fields_count: e.variants.len(),
        methods_count: 0,
        derives: {
            let mut m = HashMap::new();
            collect_derives(&e.attrs, &mut m);
            m.keys().cloned().collect()
        },
        has_generics: !e.generics.params.is_empty(),
        visibility: vis_string(&e.vis),
    });
}

fn process_impl_item(ctx: &mut RsItemCtx<'_>, i: &syn::ItemImpl) {
    if let Type::Path(tp) = &*i.self_ty {
        let parent = tp.path.segments.last().map(|s| s.ident.to_string());
        for impl_item in &i.items {
            if let ImplItem::Fn(m) = impl_item {
                collect_allow_lints(&m.attrs, ctx.allow_lints);
                let body = &m.block;
                *ctx.unsafe_blocks += count_unsafe_blocks(body);
                *ctx.unwrap_expect += count_unwrap_expect_in_block(body);
                if matches!(m.vis, Visibility::Public(_)) {
                    if let Some(p) = &parent {
                        ctx.exports.push(format!("{p}::{}", m.sig.ident));
                    }
                }
                push_fn(
                    ctx.functions,
                    RsFnSpec {
                        sig: &m.sig,
                        vis: &m.vis,
                        block: Some(body),
                        file: ctx.rel_file,
                        parent_type: parent.clone(),
                        is_method: true,
                    },
                );
            }
        }
    }
}

fn process_expr_audits(ctx: &mut RsItemCtx<'_>, expr: &Expr) {
    if let Expr::Block(eb) = expr {
        *ctx.unsafe_blocks += count_unsafe_blocks(&eb.block);
        *ctx.unwrap_expect += count_unwrap_expect_in_block(&eb.block);
    } else {
        *ctx.unwrap_expect += count_unwrap_expect_expr(expr);
        *ctx.unsafe_blocks += count_unsafe_expr(expr);
    }
}

fn process_item(ctx: &mut RsItemCtx<'_>, item: &Item) {
    collect_allow_lints(item_attrs(item), ctx.allow_lints);
    collect_derives(item_attrs(item), ctx.derive_hits);

    match item {
        Item::Fn(f) => process_fn_item(ctx, f),
        Item::Struct(s) => process_struct_item(ctx, s),
        Item::Enum(e) => process_enum_item(ctx, e),
        Item::Trait(t) => {
            if matches!(t.vis, Visibility::Public(_)) {
                ctx.exports.push(t.ident.to_string());
            }
            ctx.traits.push(RsTraitInfo {
                name: t.ident.to_string(),
                file: ctx.rel_file.to_string(),
                line: span_line(t.trait_token.span),
                visibility: vis_string(&t.vis),
            });
        }
        Item::Impl(i) => process_impl_item(ctx, i),
        Item::Use(u) => {
            process_use_item(ctx.current_mod, u, ctx.imports, ctx.exports)
        }
        Item::Mod(m) => {
            if let Some((_, items)) = &m.content {
                for inner in items {
                    process_item(ctx, inner);
                }
            }
        }
        Item::Const(c) => {
            if matches!(c.vis, Visibility::Public(_)) {
                ctx.exports.push(c.ident.to_string());
            }
            process_expr_audits(ctx, c.expr.as_ref());
        }
        Item::Static(s) => {
            if matches!(s.vis, Visibility::Public(_)) {
                ctx.exports.push(s.ident.to_string());
            }
            process_expr_audits(ctx, &s.expr);
        }
        Item::Type(t) => {
            if matches!(t.vis, Visibility::Public(_)) {
                ctx.exports.push(t.ident.to_string());
            }
        }
        _ => {}
    }
}

pub(crate) fn analyze_rs_file(
    path: &Path,
    scan_root: &Path,
    exclude: &[String],
) -> Option<Result<RsFileData, (String, String)>> {
    if !exclude.is_empty() && matches_exclude(path, scan_root, exclude) {
        return None;
    }
    let source = match fs::read_to_string(path) {
        Ok(s) => s.replace('\r', ""),
        Err(_) => return None,
    };
    let rel = display_rel(path, scan_root);
    let abs = path.display().to_string();
    let line_count = source.matches('\n').count() + 1;
    let current_mod = rust_file_to_module(&rel);

    let ast = match syn::parse_file(&source) {
        Ok(f) => f,
        Err(e) => {
            return Some(Err((rel, e.to_string())));
        }
    };

    let mut functions = Vec::new();
    let mut structs = Vec::new();
    let mut traits = Vec::new();
    let mut imports = Vec::new();
    let mut exports = Vec::new();
    let mut allow_lints = HashMap::new();
    let mut derive_hits = HashMap::new();
    let mut unsafe_blocks = 0usize;
    let mut unwrap_expect = 0usize;

    let mut ctx = RsItemCtx {
        current_mod: current_mod.as_str(),
        rel_file: &rel,
        functions: &mut functions,
        structs: &mut structs,
        traits: &mut traits,
        imports: &mut imports,
        exports: &mut exports,
        allow_lints: &mut allow_lints,
        derive_hits: &mut derive_hits,
        unsafe_blocks: &mut unsafe_blocks,
        unwrap_expect: &mut unwrap_expect,
    };
    for item in &ast.items {
        process_item(&mut ctx, item);
    }

    // Fill methods_count on structs from impl methods
    for f in &functions {
        if let Some(parent) = &f.parent_type {
            if let Some(st) = structs.iter_mut().find(|s| s.name == *parent) {
                st.methods_count += 1;
            }
        }
    }

    Some(Ok(RsFileData {
        rel_path: rel,
        abs_path: abs,
        line_count,
        source,
        functions,
        structs,
        traits,
        imports,
        exports,
        unsafe_blocks,
        unwrap_expect_count: unwrap_expect,
        allow_lint_hits: allow_lints,
        derive_hits,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_file_to_module_strips_rs_and_mod_rs() {
        assert_eq!(rust_file_to_module("src/foo/bar.rs"), "src/foo/bar");
        assert_eq!(rust_file_to_module("foo/mod.rs"), "foo");
    }
}
