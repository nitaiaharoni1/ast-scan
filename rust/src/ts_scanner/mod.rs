//! TypeScript/JavaScript scanner (OXC); JSON and behavior aligned with the original TS CLI.
//! Per-file analysis runs on a rayon thread pool (`par_iter`); aggregation is sequential.

mod file;
mod visitors;

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::Context;
use rayon::prelude::*;
use serde_json::{json, Value};

use crate::audits::{collect_eslint_disables, collect_todo_comments, collect_ts_directives};
use crate::graph::{compute_coupling, find_cycles, unique_cycles};
use crate::types::TsFileData;

fn rel_from_scan_root(data: &TsFileData, root: &Path) -> String {
    let abs = Path::new(&data.abs_path);
    abs.strip_prefix(root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| data.rel_path.replace('\\', "/"))
}

#[derive(Debug, Default, Clone)]
pub(crate) struct AnalysisConfig {
    pub orm_check_methods: Option<Vec<String>>,
    pub boundary_rules: Vec<crate::types::BoundaryRule>,
    pub exclude: Vec<String>,
}

/// Text report `--skip` sections (same names as the historical TypeScript CLI).
pub(crate) fn ts_text_skip_sections() -> HashSet<&'static str> {
    [
        "inventory",
        "complexity",
        "nesting",
        "imports",
        "coupling",
        "cycles",
        "dead-exports",
        "component-props",
        "hooks",
        "silent-catches",
        "eslint-disables",
        "ts-directives",
        "any-audit",
        "todo-audit",
        "console-debugger",
        "mobx-observer",
        "orm-case-check",
        "import-boundaries",
        "cognitive",
        "code-clones",
        "security-audit",
        "test-prod",
    ]
    .into_iter()
    .collect()
}

struct TsImportGraph {
    graph: HashMap<String, HashSet<String>>,
    in_degree: HashMap<String, usize>,
    all_modules: HashSet<String>,
    imported_names_map: HashMap<String, HashSet<String>>,
}

fn build_import_graph(all_data: &[TsFileData], root: &Path) -> TsImportGraph {
    let mut graph: HashMap<String, HashSet<String>> = HashMap::new();
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    let mut all_modules: HashSet<String> = HashSet::new();
    let mut imported_names_map: HashMap<String, HashSet<String>> = HashMap::new();

    for d in all_data {
        let rel = rel_from_scan_root(d, root);
        let from_mod = file::normalize_module_path(&rel);
        all_modules.insert(from_mod);
    }

    for d in all_data {
        let rel = rel_from_scan_root(d, root);
        let from_mod = file::normalize_module_path(&rel);

        for imp in &d.imports {
            if !imp.is_internal {
                continue;
            }
            let to_mod = file::normalize_module_path(&imp.resolved_path);
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

    TsImportGraph {
        graph,
        in_degree,
        all_modules,
        imported_names_map,
    }
}

fn find_dead_exports(
    all_data: &[TsFileData],
    root: &Path,
    imported_names_map: &HashMap<String, HashSet<String>>,
) -> Vec<(String, String)> {
    let index_re = regex::Regex::new(r"/index\.(tsx?|jsx?)$")
        .expect("static index route regex for dead-export heuristic");
    let entry_points: HashSet<&str> = ["App", "main"].into_iter().collect();
    let mut dead_exports = Vec::new();
    for d in all_data {
        let rel = &d.rel_path;
        let rel_norm = file::normalize_module_path(&rel_from_scan_root(d, root));
        let base_name = Path::new(rel)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if entry_points.contains(base_name) || index_re.is_match(rel) {
            continue;
        }
        let imported = imported_names_map
            .get(&rel_norm)
            .cloned()
            .unwrap_or_default();
        for exp in &d.exports {
            if !imported.contains(exp) && !exp.starts_with('_') {
                dead_exports.push((rel_norm.clone(), exp.clone()));
            }
        }
    }
    dead_exports.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    dead_exports
}

type RuleFileMap = HashMap<String, (usize, Vec<String>)>;

struct TsTextAudits {
    eslint_map: RuleFileMap,
    ts_dir_map: RuleFileMap,
    todo_freq: HashMap<String, usize>,
    todo_samples: HashMap<String, Vec<String>>,
}

fn collect_text_audits(all_data: &[TsFileData]) -> TsTextAudits {
    let mut eslint_map: RuleFileMap = HashMap::new();
    let mut ts_dir_map: RuleFileMap = HashMap::new();
    let mut todo_freq: HashMap<String, usize> = HashMap::new();
    let mut todo_samples: HashMap<String, Vec<String>> = HashMap::new();
    for d in all_data {
        collect_eslint_disables(&d.source, &d.rel_path, &mut eslint_map);
        collect_ts_directives(&d.source, &d.rel_path, &mut ts_dir_map);
        collect_todo_comments(&d.source, &d.rel_path, &mut todo_freq, &mut todo_samples);
    }
    TsTextAudits { eslint_map, ts_dir_map, todo_freq, todo_samples }
}

fn build_ext_freq(all_data: &[TsFileData]) -> Vec<(String, usize)> {
    let all_imports: Vec<_> = all_data.iter().flat_map(|d| &d.imports).collect();
    let mut ext_freq: HashMap<String, usize> = HashMap::new();
    for imp in all_imports.iter().filter(|i| !i.is_internal) {
        let pkg = if imp.source.starts_with('@') {
            imp.source.split('/').take(2).collect::<Vec<_>>().join("/")
        } else {
            imp.source
                .split('/')
                .next()
                .unwrap_or(&imp.source)
                .to_string()
        };
        *ext_freq.entry(pkg).or_insert(0) += 1;
    }
    let mut sorted: Vec<_> = ext_freq.into_iter().collect::<Vec<_>>();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    sorted
}

struct TsAggregated {
    ig: TsImportGraph,
    dead_exports: Vec<(String, String)>,
    ext_sorted: Vec<(String, usize)>,
    audits: TsTextAudits,
}

const CLONE_MIN_LINES_TS: usize = 10;

fn build_code_clones_ts(funcs: &[crate::types::TsFuncInfo]) -> Vec<Value> {
    let mut m: HashMap<u64, Vec<&crate::types::TsFuncInfo>> = HashMap::new();
    for f in funcs {
        if f.line_count > CLONE_MIN_LINES_TS {
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
                    "name": f.name,
                    "file": f.file,
                    "line": f.line,
                    "lines": f.line_count,
                })).collect::<Vec<_>>()
            })
        })
        .collect()
}

fn build_json(
    all_data: &[TsFileData],
    root: &Path,
    alias_prefix: &str,
    config: &AnalysisConfig,
    agg: TsAggregated,
) -> anyhow::Result<Value> {
    let all_functions: Vec<_> = all_data.iter().flat_map(|d| d.functions.clone()).collect();
    let all_classes: Vec<_> = all_data.iter().flat_map(|d| d.classes.clone()).collect();
    let all_imports: Vec<_> = all_data.iter().flat_map(|d| d.imports.clone()).collect();

    let components: Vec<_> = all_functions.iter().filter(|f| f.is_component).cloned().collect();
    let custom_hook_fns: Vec<_> = all_functions
        .iter()
        .filter(|f| {
            f.name.len() > 3
                && f.name.starts_with("use")
                && f.name.as_bytes().get(3).is_some_and(|b| b.is_ascii_uppercase())
                && f.exported
        })
        .cloned()
        .collect();

    let coupling_rows = compute_coupling(&agg.ig.graph, &agg.ig.all_modules);
    let coupling_json: Vec<Value> = coupling_rows
        .iter()
        .map(|r| json!({"module": r.module, "ca": r.ca, "ce": r.ce, "instability": r.instability}))
        .collect();

    let mut top_imported: Vec<_> = agg.ig.in_degree.iter().collect::<Vec<_>>();
    top_imported.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    let top_imported_json: Vec<_> = top_imported
        .iter()
        .map(|(m, c)| json!({"module": m, "count": c}))
        .collect();

    let total_edges: usize = agg.ig.graph.values().map(|s| s.len()).sum();
    let raw_cycles = find_cycles(&agg.ig.graph);
    let cycles_unique = unique_cycles(&raw_cycles);
    let cycles_str: Vec<String> = cycles_unique.iter().map(|c| c.join(" -> ")).collect();

    let mut hook_freq: HashMap<String, usize> = HashMap::new();
    for c in &components {
        for h in &c.hooks {
            *hook_freq.entry(h.clone()).or_insert(0) += 1;
        }
    }

    let mut all_console: Vec<_> = all_data.iter().flat_map(|d| d.console_debugger.clone()).collect();
    all_console.sort_by(|a, b| a.file.cmp(&b.file).then_with(|| a.line.cmp(&b.line)));
    let mut console_kind_freq: HashMap<String, usize> = HashMap::new();
    for item in &all_console {
        *console_kind_freq.entry(item.kind.clone()).or_insert(0) += 1;
    }

    let mut all_silent: Vec<_> = all_data.iter().flat_map(|d| d.silent_catches.clone()).collect();
    all_silent.sort_by(|a, b| a.file.cmp(&b.file).then_with(|| a.line.cmp(&b.line)));

    let mut eslint_sorted: Vec<_> = agg.audits.eslint_map
        .into_iter()
        .map(|(rule, (count, files))| json!({"rule": rule, "count": count, "files": files}))
        .collect();
    eslint_sorted.sort_by(|a, b| b["count"].as_u64().unwrap_or(0).cmp(&a["count"].as_u64().unwrap_or(0)).then_with(|| a["rule"].as_str().cmp(&b["rule"].as_str())));
    let eslint_total: usize = eslint_sorted.iter().filter_map(|v| v["count"].as_u64()).map(|u| u as usize).sum();

    let mut ts_sorted: Vec<_> = agg.audits.ts_dir_map
        .into_iter()
        .map(|(directive, (count, files))| json!({"directive": directive, "count": count, "files": files}))
        .collect();
    ts_sorted.sort_by(|a, b| b["count"].as_u64().unwrap_or(0).cmp(&a["count"].as_u64().unwrap_or(0)).then_with(|| a["directive"].as_str().cmp(&b["directive"].as_str())));
    let ts_total: usize = ts_sorted.iter().filter_map(|v| v["count"].as_u64()).map(|u| u as usize).sum();

    let mut any_rows: Vec<_> = all_data
        .iter()
        .filter(|d| d.any_count > 0)
        .map(|d| json!({"file": d.rel_path, "count": d.any_count}))
        .collect();
    any_rows.sort_by(|a, b| b["count"].as_u64().unwrap_or(0).cmp(&a["count"].as_u64().unwrap_or(0)).then_with(|| a["file"].as_str().cmp(&b["file"].as_str())));
    let any_total: usize = all_data.iter().map(|d| d.any_count).sum();

    let todo_total: usize = agg.audits.todo_freq.values().sum();
    let mut todo_by_tag: Vec<_> = agg.audits.todo_freq
        .iter()
        .map(|(tag, count)| json!({"tag": tag, "count": count, "samples": agg.audits.todo_samples.get(tag).cloned().unwrap_or_default()}))
        .collect();
    todo_by_tag.sort_by(|a, b| b["count"].as_u64().unwrap_or(0).cmp(&a["count"].as_u64().unwrap_or(0)).then_with(|| a["tag"].as_str().cmp(&b["tag"].as_str())));

    let mut all_mobx: Vec<_> = all_data.iter().flat_map(|d| d.mobx_observer_issues.clone()).collect();
    all_mobx.sort_by(|a, b| a.file.cmp(&b.file).then_with(|| a.line.cmp(&b.line)));

    let mut all_orm: Vec<_> = all_data.iter().flat_map(|d| d.orm_case_issues.clone()).collect();
    all_orm.sort_by(|a, b| a.file.cmp(&b.file).then_with(|| a.line.cmp(&b.line)));

    let boundary_violations = if !config.boundary_rules.is_empty() {
        file::check_boundaries(all_data, &config.boundary_rules)
    } else {
        Vec::new()
    };

    let total_lines: usize = all_data.iter().map(|d| d.line_count).sum();
    let internal_imports = all_imports.iter().filter(|i| i.is_internal).count();
    let external_imports = all_imports.iter().filter(|i| !i.is_internal).count();

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

    let code_clones = build_code_clones_ts(&all_functions);

    let mut complexity_rows: Vec<_> = all_functions
        .iter()
        .map(|fn_| json!({
            "name": fn_.name,
            "cc": fn_.complexity,
            "cognitive": fn_.cognitive_complexity,
            "params": fn_.param_count,
            "nesting": fn_.nesting,
            "file": fn_.file,
            "line": fn_.line,
            "is_component": fn_.is_component,
            "is_test": fn_.is_test,
        }))
        .collect();
    complexity_rows.sort_by(|a, b| b["cc"].as_u64().unwrap_or(0).cmp(&a["cc"].as_u64().unwrap_or(0)).then_with(|| a["name"].as_str().cmp(&b["name"].as_str())));

    let mut cognitive_rows: Vec<_> = all_functions
        .iter()
        .map(|fn_| json!({
            "name": fn_.name,
            "cognitive": fn_.cognitive_complexity,
            "file": fn_.file,
            "line": fn_.line,
            "is_component": fn_.is_component,
        }))
        .collect();
    cognitive_rows.sort_by(|a, b| {
        b["cognitive"]
            .as_u64()
            .unwrap_or(0)
            .cmp(&a["cognitive"].as_u64().unwrap_or(0))
            .then_with(|| a["name"].as_str().cmp(&b["name"].as_str()))
    });

    let mut nesting_rows: Vec<_> = all_functions
        .iter()
        .filter(|f| f.nesting > 0)
        .map(|fn_| json!({"name": fn_.name, "depth": fn_.nesting, "file": fn_.file, "line": fn_.line, "is_component": fn_.is_component}))
        .collect();
    nesting_rows.sort_by(|a, b| b["depth"].as_u64().unwrap_or(0).cmp(&a["depth"].as_u64().unwrap_or(0)).then_with(|| a["name"].as_str().cmp(&b["name"].as_str())));

    let mut files_by_lines: Vec<_> = all_data
        .iter()
        .map(|d| json!({"file": d.rel_path, "lines": d.line_count}))
        .collect();
    files_by_lines.sort_by(|a, b| b["lines"].as_u64().unwrap_or(0).cmp(&a["lines"].as_u64().unwrap_or(0)).then_with(|| a["file"].as_str().cmp(&b["file"].as_str())));

    let mut largest_fn: Vec<_> = all_functions
        .iter()
        .map(|fn_| json!({"name": fn_.name, "lines": fn_.line_count, "file": fn_.file, "line": fn_.line, "is_component": fn_.is_component}))
        .collect();
    largest_fn.sort_by(|a, b| b["lines"].as_u64().unwrap_or(0).cmp(&a["lines"].as_u64().unwrap_or(0)).then_with(|| a["name"].as_str().cmp(&b["name"].as_str())));

    let mut largest_cls: Vec<_> = all_classes
        .iter()
        .map(|cls| json!({"name": cls.name, "lines": cls.line_count, "methods": cls.methods, "properties": cls.properties, "file": cls.file, "line": cls.line, "has_heritage": cls.has_heritage}))
        .collect();
    largest_cls.sort_by(|a, b| b["lines"].as_u64().unwrap_or(0).cmp(&a["lines"].as_u64().unwrap_or(0)).then_with(|| a["name"].as_str().cmp(&b["name"].as_str())));

    let component_props: Vec<_> = components
        .iter()
        .filter(|c| !c.props.is_empty())
        .map(|c| json!({"name": c.name, "file": c.file, "line": c.line, "props": c.props}))
        .collect();

    let mut hook_freq_sorted: Vec<_> = hook_freq.into_iter().collect();
    hook_freq_sorted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let hook_frequency: Vec<_> = hook_freq_sorted
        .into_iter()
        .map(|(hook, count)| {
            let is_custom = custom_hook_fns.iter().any(|f| f.name == hook);
            json!({"hook": hook, "count": count, "is_custom": is_custom})
        })
        .collect();

    let mut custom_inv: Vec<_> = custom_hook_fns
        .iter()
        .map(|fn_| json!({"name": fn_.name, "lines": fn_.line_count, "file": fn_.file, "line": fn_.line}))
        .collect();
    custom_inv.sort_by(|a, b| a["name"].as_str().unwrap_or("").cmp(b["name"].as_str().unwrap_or("")));

    let mut heavy: Vec<_> = components
        .iter()
        .filter(|c| c.hooks.len() >= 3)
        .map(|c| json!({"name": c.name, "file": c.file, "line": c.line, "hook_count": c.hooks.len(), "hooks": c.hooks}))
        .collect();
    heavy.sort_by(|a, b| b["hook_count"].as_u64().unwrap_or(0).cmp(&a["hook_count"].as_u64().unwrap_or(0)).then_with(|| a["name"].as_str().cmp(&b["name"].as_str())));

    let mut by_kind_console: Vec<_> = console_kind_freq
        .into_iter()
        .map(|(kind, count)| json!({"kind": kind, "count": count}))
        .collect();
    by_kind_console.sort_by(|a, b| b["count"].as_u64().unwrap_or(0).cmp(&a["count"].as_u64().unwrap_or(0)).then_with(|| a["kind"].as_str().cmp(&b["kind"].as_str())));

    let orm_json = if let Some(ref methods) = config.orm_check_methods {
        if !methods.is_empty() {
            json!({"methods": methods, "findings": serde_json::to_value(&all_orm)?})
        } else {
            Value::Null
        }
    } else {
        Value::Null
    };

    let import_boundaries = if !config.boundary_rules.is_empty() {
        json!({"rules": serde_json::to_value(&config.boundary_rules)?, "violations": serde_json::to_value(&boundary_violations)?})
    } else {
        Value::Null
    };

    Ok(json!({
        "scanner": "typescript",
        "scan_root": root.display().to_string(),
        "alias_prefix": alias_prefix,
        "summary": {
            "files": all_data.len(),
            "lines": total_lines,
            "functions": all_functions.len(),
            "classes": all_classes.len(),
            "components": components.len(),
            "custom_hooks": custom_hook_fns.len(),
            "internal_imports": internal_imports,
            "external_imports": external_imports,
            "test_prod": {
                "test_lines": test_lines,
                "production_lines": prod_lines,
                "test_functions": test_functions,
                "production_functions": prod_functions,
                "line_ratio_test": line_ratio_test,
                "function_ratio_test": fn_ratio_test,
            },
        },
        "inventory": {
            "files_by_lines": files_by_lines,
            "largest_functions": largest_fn,
            "largest_classes": largest_cls,
        },
        "complexity": complexity_rows,
        "cognitive": cognitive_rows,
        "nesting": nesting_rows,
        "code_clones": code_clones,
        "security_audit": {
            "total": all_security.len(),
            "findings": serde_json::to_value(&all_security)?,
        },
        "imports": {
            "modules": agg.ig.all_modules.len(),
            "edges": total_edges,
            "top_imported": top_imported_json,
            "external_packages": agg.ext_sorted.iter().map(|(p, c)| json!({"package": p, "count": c})).collect::<Vec<_>>(),
        },
        "coupling": coupling_json,
        "cycles": cycles_str,
        "cycles_raw": cycles_unique,
        "dead_exports": agg.dead_exports.iter().map(|(m, n)| json!({"module": m, "name": n})).collect::<Vec<_>>(),
        "component_props": component_props,
        "hooks": {
            "frequency": hook_frequency,
            "custom_hooks_inventory": custom_inv,
            "heavy_components": heavy,
        },
        "console_debugger": {
            "total": all_console.len(),
            "by_kind": by_kind_console,
            "items": serde_json::to_value(&all_console)?,
        },
        "silent_catches": serde_json::to_value(&all_silent)?,
        "eslint_disables": {
            "total": eslint_total,
            "unique_rules": eslint_sorted.len(),
            "by_rule": eslint_sorted,
        },
        "any_audit": {
            "total": any_total,
            "by_file": any_rows,
        },
        "ts_directives": {
            "total": ts_total,
            "by_directive": ts_sorted,
        },
        "todo_audit": {
            "total": todo_total,
            "by_tag": todo_by_tag,
        },
        "mobx_observer": serde_json::to_value(&all_mobx)?,
        "orm_case_check": orm_json,
        "import_boundaries": import_boundaries,
    }))
}

pub(crate) fn analyze_typescript(
    scan_root: &Path,
    alias_prefix: &str,
    config: &AnalysisConfig,
) -> anyhow::Result<Value> {
    let root = scan_root
        .canonicalize()
        .with_context(|| format!("cannot resolve {}", scan_root.display()))?;
    if !root.is_dir() {
        anyhow::bail!("Not a directory: {}", root.display());
    }

    let exclude = &config.exclude[..];
    let orm_set: Option<HashSet<String>> = config.orm_check_methods.as_ref().map(|v| {
        v.iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    });

    let files = file::collect_ts_files(&root, exclude);
    let all_data: Vec<TsFileData> = files
        .par_iter()
        .filter_map(|f| file::analyze_ts_file(f, &root, alias_prefix, orm_set.as_ref(), exclude))
        .collect();

    let ig = build_import_graph(&all_data, &root);
    let dead_exports = find_dead_exports(&all_data, &root, &ig.imported_names_map);
    let ext_sorted = build_ext_freq(&all_data);
    let audits = collect_text_audits(&all_data);

    build_json(
        &all_data,
        &root,
        alias_prefix,
        config,
        TsAggregated { ig, dead_exports, ext_sorted, audits },
    )
}
