//! Rust source scanner (`syn`).
//! Per-file work uses rayon (`par_iter`), like Python and TypeScript modes.

mod file;
mod visitors;

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::Context;
use rayon::prelude::*;
use serde_json::{json, Value};

use crate::audits::collect_todo_comments;
use crate::graph::{compute_coupling, find_cycles, unique_cycles};
use crate::types::RsFileData;

fn rel_module_id(data: &RsFileData) -> String {
    file::rust_file_to_module(&data.rel_path)
}

/// Text report `--skip` section names for Rust mode.
pub(crate) fn rs_text_skip_sections() -> HashSet<&'static str> {
    [
        "inventory",
        "complexity",
        "nesting",
        "imports",
        "coupling",
        "cycles",
        "dead-exports",
        "unsafe-audit",
        "unwrap-audit",
        "allow-lints",
        "derive-audit",
        "todo-audit",
        "traits",
        "parse-errors",
        "cognitive",
        "code-clones",
        "security-audit",
        "test-prod",
    ]
    .into_iter()
    .collect()
}

struct CollectedData {
    all_data: Vec<RsFileData>,
    parse_errors: Vec<Value>,
}

fn collect_and_parse(root: &Path, exclude: &[String]) -> CollectedData {
    let paths = file::collect_rs_files(root, exclude);
    let results: Vec<Result<RsFileData, (String, String)>> = paths
        .par_iter()
        .filter_map(|p| file::analyze_rs_file(p, root, exclude))
        .collect();

    let mut all_data: Vec<RsFileData> = Vec::new();
    let mut parse_errors: Vec<Value> = Vec::new();
    for r in results {
        match r {
            Ok(d) => all_data.push(d),
            Err((rel, msg)) => {
                parse_errors.push(json!({ "file": rel, "message": msg }));
            }
        }
    }
    parse_errors.sort_by(|a, b| {
        let fa = a["file"].as_str().unwrap_or("");
        let fb = b["file"].as_str().unwrap_or("");
        fa.cmp(fb)
    });
    all_data.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    CollectedData {
        all_data,
        parse_errors,
    }
}

struct ImportGraph {
    graph: HashMap<String, HashSet<String>>,
    in_degree: HashMap<String, usize>,
    all_modules: HashSet<String>,
    imported_names_map: HashMap<String, HashSet<String>>,
}

fn build_import_graph(all_data: &[RsFileData]) -> ImportGraph {
    let mut graph: HashMap<String, HashSet<String>> = HashMap::new();
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    let mut all_modules: HashSet<String> = HashSet::new();
    let mut imported_names_map: HashMap<String, HashSet<String>> = HashMap::new();

    for d in all_data {
        all_modules.insert(rel_module_id(d));
    }

    for d in all_data {
        let from_mod = rel_module_id(d);
        for imp in &d.imports {
            if !imp.is_internal || imp.resolved_path.is_empty() {
                continue;
            }
            let to_mod = imp.resolved_path.clone();
            graph
                .entry(from_mod.clone())
                .or_default()
                .insert(to_mod.clone());
            *in_degree.entry(to_mod.clone()).or_insert(0) += 1;
            let entry = imported_names_map.entry(to_mod).or_default();
            for s in &imp.specifiers {
                entry.insert(s.clone());
            }
        }
    }

    ImportGraph {
        graph,
        in_degree,
        all_modules,
        imported_names_map,
    }
}

fn build_ext_crate_freq(all_data: &[RsFileData]) -> Vec<(String, usize)> {
    let all_imports: Vec<_> = all_data.iter().flat_map(|d| &d.imports).collect();
    let mut ext_crate_freq: HashMap<String, usize> = HashMap::new();
    for imp in &all_imports {
        if imp.is_internal {
            continue;
        }
        let Some(first) = imp.source.split("::").next() else {
            continue;
        };
        if !first.is_empty() {
            *ext_crate_freq.entry(first.to_string()).or_insert(0) += 1;
        }
    }
    let mut sorted: Vec<_> = ext_crate_freq.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    sorted
}

fn find_dead_exports(all_data: &[RsFileData], imported_names_map: &HashMap<String, HashSet<String>>) -> Vec<(String, String)> {
    let entry_points: HashSet<&str> = ["main", "lib", "mod"].into_iter().collect();
    let mut dead_exports = Vec::new();
    for d in all_data {
        let rel_norm = rel_module_id(d);
        let base = Path::new(&d.rel_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if entry_points.contains(base) {
            continue;
        }
        let imported = imported_names_map
            .get(&rel_norm)
            .cloned()
            .unwrap_or_default();
        for exp in &d.exports {
            if exp == "*" {
                continue;
            }
            let check = exp
                .rsplit_once("::")
                .map(|(_, s)| s)
                .unwrap_or(exp.as_str());
            if !imported.contains(check) && !check.starts_with('_') {
                dead_exports.push((rel_norm.clone(), exp.clone()));
            }
        }
    }
    dead_exports.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    dead_exports
}

fn collect_rust_audits(all_data: &[RsFileData]) -> Value {
    let mut allow_global: HashMap<String, usize> = HashMap::new();
    let mut derive_global: HashMap<String, usize> = HashMap::new();
    let mut unsafe_by_file: Vec<Value> = Vec::new();
    let mut unwrap_by_file: Vec<Value> = Vec::new();
    let mut total_unsafe_blocks = 0usize;
    let mut unsafe_fn_count = 0usize;

    for d in all_data {
        total_unsafe_blocks += d.unsafe_blocks;
        for f in &d.functions {
            if f.is_unsafe {
                unsafe_fn_count += 1;
            }
        }
        if d.unsafe_blocks > 0 || d.functions.iter().any(|f| f.is_unsafe) {
            let ufn = d.functions.iter().filter(|f| f.is_unsafe).count();
            unsafe_by_file.push(json!({
                "file": d.rel_path,
                "unsafe_blocks": d.unsafe_blocks,
                "unsafe_functions": ufn,
            }));
        }
        if d.unwrap_expect_count > 0 {
            unwrap_by_file.push(json!({
                "file": d.rel_path,
                "count": d.unwrap_expect_count,
            }));
        }
        for (k, v) in &d.allow_lint_hits {
            *allow_global.entry(k.clone()).or_insert(0) += v;
        }
        for (k, v) in &d.derive_hits {
            *derive_global.entry(k.clone()).or_insert(0) += v;
        }
    }
    unsafe_by_file.sort_by(|a, b| {
        let ca =
            a["unsafe_blocks"].as_u64().unwrap_or(0) + a["unsafe_functions"].as_u64().unwrap_or(0);
        let cb =
            b["unsafe_blocks"].as_u64().unwrap_or(0) + b["unsafe_functions"].as_u64().unwrap_or(0);
        cb.cmp(&ca)
            .then_with(|| a["file"].as_str().cmp(&b["file"].as_str()))
    });
    unwrap_by_file.sort_by(|a, b| {
        b["count"]
            .as_u64()
            .unwrap_or(0)
            .cmp(&a["count"].as_u64().unwrap_or(0))
            .then_with(|| a["file"].as_str().cmp(&b["file"].as_str()))
    });

    let mut allow_sorted: Vec<Value> = allow_global
        .into_iter()
        .map(|(rule, count)| json!({ "rule": rule, "count": count }))
        .collect();
    allow_sorted.sort_by(|a, b| {
        b["count"]
            .as_u64()
            .unwrap_or(0)
            .cmp(&a["count"].as_u64().unwrap_or(0))
            .then_with(|| a["rule"].as_str().cmp(&b["rule"].as_str()))
    });

    let mut derive_sorted: Vec<Value> = derive_global
        .into_iter()
        .map(|(name, count)| json!({ "derive": name, "count": count }))
        .collect();
    derive_sorted.sort_by(|a, b| {
        b["count"]
            .as_u64()
            .unwrap_or(0)
            .cmp(&a["count"].as_u64().unwrap_or(0))
            .then_with(|| a["derive"].as_str().cmp(&b["derive"].as_str()))
    });

    json!({
        "unsafe_audit": {
            "unsafe_functions": unsafe_fn_count,
            "unsafe_blocks": total_unsafe_blocks,
            "by_file": unsafe_by_file,
        },
        "unwrap_audit": {
            "total": all_data.iter().map(|d| d.unwrap_expect_count).sum::<usize>(),
            "by_file": unwrap_by_file,
        },
        "allow_lints": {
            "total": allow_sorted.iter().map(|v| v["count"].as_u64().unwrap_or(0) as usize).sum::<usize>(),
            "by_rule": allow_sorted,
        },
        "derive_audit": {
            "total": derive_sorted.iter().map(|v| v["count"].as_u64().unwrap_or(0) as usize).sum::<usize>(),
            "by_derive": derive_sorted,
        },
    })
}

const CLONE_MIN_LINES_RS: usize = 10;

fn build_code_clones_rs(funcs: &[crate::types::RsFuncInfo]) -> Vec<Value> {
    let mut m: HashMap<u64, Vec<&crate::types::RsFuncInfo>> = HashMap::new();
    for f in funcs {
        if f.line_count > CLONE_MIN_LINES_RS {
            m.entry(f.clone_hash).or_default().push(f);
        }
    }
    let mut groups: Vec<_> = m.into_iter().filter(|(_, v)| v.len() > 1).collect();
    groups.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));
    groups
        .into_iter()
        .map(|(h, vs)| {
            json!({
                "hash": format!("{h:016x}"),
                "count": vs.len(),
                "functions": vs.iter().map(|f| json!({
                    "name": f.qualname,
                    "file": f.file,
                    "line": f.line,
                    "lines": f.line_count,
                })).collect::<Vec<_>>()
            })
        })
        .collect()
}

fn build_json(
    all_data: &[RsFileData],
    parse_errors: Vec<Value>,
    ig: &ImportGraph,
    ext_sorted: &[(String, usize)],
    dead_exports: &[(String, String)],
    audit_json: Value,
    root: &Path,
) -> Value {
    let all_functions: Vec<_> = all_data.iter().flat_map(|d| d.functions.clone()).collect();
    let all_structs: Vec<_> = all_data.iter().flat_map(|d| d.structs.clone()).collect();
    let all_traits: Vec<_> = all_data.iter().flat_map(|d| d.traits.clone()).collect();
    let all_imports: Vec<_> = all_data.iter().flat_map(|d| d.imports.clone()).collect();

    let coupling_rows = compute_coupling(&ig.graph, &ig.all_modules);
    let coupling_json: Vec<Value> = coupling_rows
        .iter()
        .map(|r| json!({"module": r.module, "ca": r.ca, "ce": r.ce, "instability": r.instability}))
        .collect();

    let mut top_imported: Vec<_> = ig.in_degree.iter().collect::<Vec<_>>();
    top_imported.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    let top_imported_json: Vec<_> = top_imported
        .iter()
        .map(|(m, c)| json!({ "module": m, "count": c }))
        .collect();

    let total_edges: usize = ig.graph.values().map(|s| s.len()).sum();
    let raw_cycles = find_cycles(&ig.graph);
    let cycles_unique = unique_cycles(&raw_cycles);
    let cycles_str: Vec<String> = cycles_unique.iter().map(|c| c.join(" -> ")).collect();

    let test_lines: usize = all_data
        .iter()
        .filter(|d| d.is_test_file)
        .map(|d| d.line_count)
        .sum();
    let prod_lines: usize = all_data
        .iter()
        .filter(|d| !d.is_test_file)
        .map(|d| d.line_count)
        .sum();
    let test_functions = all_functions.iter().filter(|f| f.is_test).count();
    let prod_functions = all_functions.len() - test_functions;
    let line_total_tp = test_lines + prod_lines;
    let line_ratio_test = if line_total_tp > 0 {
        test_lines as f64 / line_total_tp as f64
    } else {
        0.0
    };
    let fn_total_tp = test_functions + prod_functions;
    let fn_ratio_test = if fn_total_tp > 0 {
        test_functions as f64 / fn_total_tp as f64
    } else {
        0.0
    };

    let mut all_security: Vec<_> = all_data
        .iter()
        .flat_map(|d| d.security_findings.iter().cloned())
        .collect();
    all_security.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then_with(|| a.line.cmp(&b.line))
            .then_with(|| a.kind.cmp(&b.kind))
    });

    let code_clones = build_code_clones_rs(&all_functions);

    let mut complexity_rows: Vec<Value> = all_functions
        .iter()
        .map(|f| json!({
            "name": f.qualname,
            "cc": f.complexity,
            "cognitive": f.cognitive_complexity,
            "params": f.param_count,
            "nesting": f.nesting,
            "file": f.file,
            "line": f.line,
            "is_method": f.is_method,
            "is_unsafe": f.is_unsafe,
            "is_test": f.is_test,
        }))
        .collect();
    complexity_rows.sort_by(|a, b| b["cc"].as_u64().unwrap_or(0).cmp(&a["cc"].as_u64().unwrap_or(0)).then_with(|| a["name"].as_str().cmp(&b["name"].as_str())));

    let mut cognitive_rows: Vec<Value> = all_functions
        .iter()
        .map(|f| json!({
            "name": f.qualname,
            "cognitive": f.cognitive_complexity,
            "file": f.file,
            "line": f.line,
            "is_method": f.is_method,
        }))
        .collect();
    cognitive_rows.sort_by(|a, b| {
        b["cognitive"]
            .as_u64()
            .unwrap_or(0)
            .cmp(&a["cognitive"].as_u64().unwrap_or(0))
            .then_with(|| a["name"].as_str().cmp(&b["name"].as_str()))
    });

    let mut nesting_rows: Vec<Value> = all_functions
        .iter()
        .filter(|f| f.nesting > 0)
        .map(|f| json!({"name": f.qualname, "depth": f.nesting, "file": f.file, "line": f.line, "is_method": f.is_method}))
        .collect();
    nesting_rows.sort_by(|a, b| b["depth"].as_u64().unwrap_or(0).cmp(&a["depth"].as_u64().unwrap_or(0)).then_with(|| a["name"].as_str().cmp(&b["name"].as_str())));

    let mut files_by_lines: Vec<Value> = all_data
        .iter()
        .map(|d| json!({"file": d.rel_path, "lines": d.line_count}))
        .collect();
    files_by_lines.sort_by(|a, b| b["lines"].as_u64().unwrap_or(0).cmp(&a["lines"].as_u64().unwrap_or(0)).then_with(|| a["file"].as_str().cmp(&b["file"].as_str())));

    let mut largest_fn: Vec<Value> = all_functions
        .iter()
        .map(|f| json!({"name": f.qualname, "lines": f.line_count, "file": f.file, "line": f.line, "is_method": f.is_method}))
        .collect();
    largest_fn.sort_by(|a, b| b["lines"].as_u64().unwrap_or(0).cmp(&a["lines"].as_u64().unwrap_or(0)).then_with(|| a["name"].as_str().cmp(&b["name"].as_str())));

    let mut largest_types: Vec<Value> = all_structs
        .iter()
        .map(|s| json!({"name": s.name, "kind": s.kind, "lines": s.line_count, "fields": s.fields_count, "methods": s.methods_count, "file": s.file, "line": s.line}))
        .collect();
    largest_types.sort_by(|a, b| b["lines"].as_u64().unwrap_or(0).cmp(&a["lines"].as_u64().unwrap_or(0)).then_with(|| a["name"].as_str().cmp(&b["name"].as_str())));

    let mut todo_freq: HashMap<String, usize> = HashMap::new();
    let mut todo_samples: HashMap<String, Vec<String>> = HashMap::new();
    for d in all_data {
        collect_todo_comments(&d.source, &d.rel_path, &mut todo_freq, &mut todo_samples);
    }
    let todo_total: usize = todo_freq.values().sum();
    let mut todo_by_tag: Vec<Value> = todo_freq
        .iter()
        .map(|(tag, count)| json!({"tag": tag, "count": count, "samples": todo_samples.get(tag.as_str()).cloned().unwrap_or_default()}))
        .collect();
    todo_by_tag.sort_by(|a, b| {
        b["count"].as_u64().unwrap_or(0).cmp(&a["count"].as_u64().unwrap_or(0))
            .then_with(|| a["tag"].as_str().cmp(&b["tag"].as_str()))
    });

    let traits_json: Vec<Value> = all_traits
        .iter()
        .map(|t| json!({"name": t.name, "file": t.file, "line": t.line, "visibility": t.visibility}))
        .collect();

    let internal_imports = all_imports.iter().filter(|i| i.is_internal).count();
    let external_imports = all_imports.len() - internal_imports;
    let total_lines: usize = all_data.iter().map(|d| d.line_count).sum();

    let mut result = json!({
        "scanner": "rust",
        "scan_root": root.display().to_string(),
        "summary": {
            "files": all_data.len(),
            "lines": total_lines,
            "functions": all_functions.len(),
            "structs_enums": all_structs.len(),
            "traits": all_traits.len(),
            "internal_imports": internal_imports,
            "external_imports": external_imports,
            "parse_errors": parse_errors.len(),
            "test_prod": {
                "test_lines": test_lines,
                "production_lines": prod_lines,
                "test_functions": test_functions,
                "production_functions": prod_functions,
                "line_ratio_test": line_ratio_test,
                "function_ratio_test": fn_ratio_test,
            },
        },
        "parse_errors": parse_errors,
        "inventory": {
            "files_by_lines": files_by_lines,
            "largest_functions": largest_fn,
            "largest_types": largest_types,
        },
        "complexity": complexity_rows,
        "cognitive": cognitive_rows,
        "nesting": nesting_rows,
        "code_clones": code_clones,
        "security_audit": {
            "total": all_security.len(),
            "findings": serde_json::to_value(&all_security).unwrap_or(Value::Null),
        },
        "imports": {
            "modules": ig.all_modules.len(),
            "edges": total_edges,
            "top_imported": top_imported_json,
            "external_crates": ext_sorted.iter().map(|(p, c)| json!({"crate": p, "count": c})).collect::<Vec<_>>(),
        },
        "coupling": coupling_json,
        "cycles": cycles_str,
        "cycles_raw": cycles_unique,
        "dead_exports": dead_exports.iter().map(|(m, n)| json!({"module": m, "name": n})).collect::<Vec<_>>(),
        "traits_inventory": traits_json,
        "todo_audit": {
            "total": todo_total,
            "by_tag": todo_by_tag,
        },
    });

    if let (Some(obj), Some(audit)) = (result.as_object_mut(), audit_json.as_object()) {
        for (k, v) in audit {
            obj.insert(k.clone(), v.clone());
        }
    }
    result
}

pub(crate) fn analyze_rust(scan_root: &Path, exclude: &[String]) -> anyhow::Result<Value> {
    let root = scan_root
        .canonicalize()
        .with_context(|| format!("cannot resolve {}", scan_root.display()))?;
    if !root.is_dir() {
        anyhow::bail!("Not a directory: {}", root.display());
    }

    let collected = collect_and_parse(&root, exclude);
    let ig = build_import_graph(&collected.all_data);
    let ext_sorted = build_ext_crate_freq(&collected.all_data);
    let dead_exports = find_dead_exports(&collected.all_data, &ig.imported_names_map);
    let audit_json = collect_rust_audits(&collected.all_data);

    Ok(build_json(
        &collected.all_data,
        collected.parse_errors,
        &ig,
        &ext_sorted,
        &dead_exports,
        audit_json,
        &root,
    ))
}
