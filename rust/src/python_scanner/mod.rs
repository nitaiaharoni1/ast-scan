//! Python package scanner (RustPython AST; behavior aligned with the original Python CLI).
//! Per-file analysis uses rayon (`par_iter`); merge into graphs and JSON is sequential.

mod file;
mod visitors;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context};
use rayon::prelude::*;

use crate::graph::{compute_coupling, find_cycles, unique_cycles};

pub(crate) fn display_rel(path: &Path, scan_root: &Path) -> String {
    if let Some(parent) = scan_root.parent() {
        if let Ok(p) = path.strip_prefix(parent) {
            return p.display().to_string();
        }
    }
    if let Ok(p) = path.strip_prefix(std::env::current_dir().unwrap_or_default()) {
        return p.display().to_string();
    }
    path.display().to_string()
}

pub(crate) fn file_to_module(fpath: &Path, scan_root: &Path, pkg: &str) -> String {
    let r = match fpath.strip_prefix(scan_root) {
        Ok(r) => r,
        Err(_) => return fpath.display().to_string(),
    };
    let mut parts: Vec<_> = r
        .with_extension("")
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if parts.last().map(|s| s.as_str()) == Some("__init__") {
        parts.pop();
    }
    if parts.is_empty() {
        return pkg.to_string();
    }
    format!("{}.{}", pkg, parts.join("."))
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

pub(crate) fn collect_py_files(scan_root: &Path, exclude: &[String]) -> Vec<PathBuf> {
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
            if ent.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                if !exclude.is_empty() && matches_exclude(&full, scan_root, exclude) {
                    continue;
                }
                stack.push(full);
            } else if name.to_string_lossy().ends_with(".py")
                && (exclude.is_empty() || !matches_exclude(&full, scan_root, exclude))
            {
                result.push(full);
            }
        }
    }
    result.sort();
    result
}

struct PyCollectedData {
    all_functions: Vec<crate::types::PyFuncInfo>,
    all_classes: Vec<crate::types::PyClassInfo>,
    all_imports: Vec<crate::types::ImportEdge>,
    all_routes: Vec<crate::types::RouteInfo>,
    module_top_names: HashMap<String, Vec<String>>,
    file_lines: HashMap<String, usize>,
    decorator_freq: HashMap<String, usize>,
    imported_names: HashMap<String, HashSet<String>>,
    all_mutable_defaults: Vec<crate::types::MutableDefaultInfo>,
    star_imported_modules: Vec<String>,
    all_silent: Vec<crate::types::SilentCatchInfo>,
    todo_freq: HashMap<String, usize>,
    todo_samples: HashMap<String, Vec<String>>,
    parse_errors: Vec<serde_json::Value>,
    file_count: usize,
    all_security: Vec<crate::types::SecurityFinding>,
    test_lines: usize,
    prod_lines: usize,
    test_functions: usize,
    prod_functions: usize,
}

fn collect_and_aggregate(
    scan_root: &Path,
    pkg: &str,
    files: &[PathBuf],
) -> PyCollectedData {
    let mut all_functions = Vec::new();
    let mut all_classes = Vec::new();
    let mut all_imports = Vec::new();
    let mut all_routes = Vec::new();
    let mut module_top_names: HashMap<String, Vec<String>> = HashMap::new();
    let mut file_lines: HashMap<String, usize> = HashMap::new();
    let mut decorator_freq: HashMap<String, usize> = HashMap::new();
    let mut imported_names: HashMap<String, HashSet<String>> = HashMap::new();
    let mut all_mutable_defaults: Vec<crate::types::MutableDefaultInfo> = Vec::new();
    let mut star_imported_modules: Vec<String> = Vec::new();
    let mut all_silent = Vec::new();
    let mut todo_freq: HashMap<String, usize> = HashMap::new();
    let mut todo_samples: HashMap<String, Vec<String>> = HashMap::new();
    let mut parse_errors: Vec<serde_json::Value> = Vec::new();
    let mut all_security: Vec<crate::types::SecurityFinding> = Vec::new();
    let mut test_lines = 0usize;
    let mut prod_lines = 0usize;
    let mut test_functions = 0usize;
    let mut prod_functions = 0usize;

    let scan_items: Vec<file::PyFileScanItem> = files
        .par_iter()
        .filter_map(|f| file::analyze_py_file(f, scan_root, pkg))
        .collect();

    for item in scan_items {
        match item {
            file::PyFileScanItem::ParseError { rel, message } => {
                parse_errors.push(serde_json::json!({"file": rel, "message": message}));
            }
            file::PyFileScanItem::Data(fd) => {
                file_lines.insert(fd.rel_path.clone(), fd.line_count);

                let fn_in_file =
                    fd.functions.len() + fd.classes.iter().map(|c| c.methods.len()).sum::<usize>();
                if fd.is_test_file {
                    test_lines += fd.line_count;
                    test_functions += fn_in_file;
                } else {
                    prod_lines += fd.line_count;
                    prod_functions += fn_in_file;
                }
                all_security.extend(fd.security_findings.iter().cloned());

                for fni in &fd.functions {
                    for d in &fni.decorators {
                        let dname = d.split('(').next().unwrap_or(d).trim_start_matches('@');
                        *decorator_freq.entry(dname.to_string()).or_insert(0) += 1;
                    }
                }
                for cls in &fd.classes {
                    for d in &cls.decorators {
                        let dname = d.split('(').next().unwrap_or(d).trim_start_matches('@');
                        *decorator_freq.entry(dname.to_string()).or_insert(0) += 1;
                    }
                }

                for edge in &fd.imports {
                    for name in &edge.names {
                        imported_names
                            .entry(edge.target_module.clone())
                            .or_default()
                            .insert(name.clone());
                    }
                }

                all_functions.extend(fd.functions);
                all_classes.extend(fd.classes);
                all_imports.extend(fd.imports);
                all_routes.extend(fd.routes);
                all_mutable_defaults.extend(fd.mutable_defaults);
                star_imported_modules.extend(fd.star_imported_modules);
                module_top_names.insert(fd.module, fd.top_level_names);
                all_silent.extend(fd.silent_excepts);

                for (tag, n) in fd.todo_freq {
                    *todo_freq.entry(tag).or_insert(0) += n;
                }
                for (tag, samples) in fd.todo_samples {
                    let list = todo_samples.entry(tag).or_default();
                    for loc in samples {
                        if list.len() < 5 && !list.contains(&loc) {
                            list.push(loc);
                        }
                    }
                }
            }
        }
    }

    parse_errors.sort_by(|a, b| {
        let fa = a.get("file").and_then(|v| v.as_str()).unwrap_or("");
        let fb = b.get("file").and_then(|v| v.as_str()).unwrap_or("");
        fa.cmp(fb).then_with(|| {
            let ma = a.get("message").and_then(|v| v.as_str()).unwrap_or("");
            let mb = b.get("message").and_then(|v| v.as_str()).unwrap_or("");
            ma.cmp(mb)
        })
    });

    PyCollectedData {
        all_functions,
        all_classes,
        all_imports,
        all_routes,
        module_top_names,
        file_lines,
        decorator_freq,
        imported_names,
        all_mutable_defaults,
        star_imported_modules,
        all_silent,
        todo_freq,
        todo_samples,
        parse_errors,
        file_count: files.len(),
        all_security,
        test_lines,
        prod_lines,
        test_functions,
        prod_functions,
    }
}

const CLONE_MIN_LINES: usize = 10;

fn build_code_clones_py(
    scan_root: &Path,
    funcs: &[crate::types::PyFuncInfo],
) -> Vec<serde_json::Value> {
    let mut m: HashMap<u64, Vec<&crate::types::PyFuncInfo>> = HashMap::new();
    for f in funcs {
        if f.line_count > CLONE_MIN_LINES {
            m.entry(f.clone_hash).or_default().push(f);
        }
    }
    let mut groups: Vec<_> = m.into_iter().filter(|(_, v)| v.len() > 1).collect();
    groups.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));
    groups
        .into_iter()
        .map(|(h, vs)| {
            serde_json::json!({
                "hash": format!("{h:016x}"),
                "count": vs.len(),
                "functions": vs.iter().map(|f| serde_json::json!({
                    "name": f.qualname,
                    "file": display_rel(Path::new(&f.file), scan_root),
                    "line": f.line,
                    "lines": f.line_count,
                })).collect::<Vec<_>>()
            })
        })
        .collect()
}

fn build_type1_clones_py(
    scan_root: &Path,
    funcs: &[crate::types::PyFuncInfo],
) -> Vec<serde_json::Value> {
    let mut m: HashMap<u64, Vec<&crate::types::PyFuncInfo>> = HashMap::new();
    for f in funcs {
        if f.line_count > CLONE_MIN_LINES {
            m.entry(f.exact_clone_hash).or_default().push(f);
        }
    }
    let mut groups: Vec<_> = m.into_iter().filter(|(_, v)| v.len() > 1).collect();
    groups.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));
    groups
        .into_iter()
        .map(|(h, vs)| {
            serde_json::json!({
                "hash": format!("{h:016x}"),
                "count": vs.len(),
                "functions": vs.iter().map(|f| serde_json::json!({
                    "name": f.qualname,
                    "file": display_rel(Path::new(&f.file), scan_root),
                    "line": f.line,
                    "lines": f.line_count,
                })).collect::<Vec<_>>()
            })
        })
        .collect()
}

fn build_json(cd: PyCollectedData, scan_root: &Path, pkg: &str) -> serde_json::Value {
    let mut graph: HashMap<String, HashSet<String>> = HashMap::new();
    for edge in &cd.all_imports {
        graph
            .entry(edge.source_module.clone())
            .or_default()
            .insert(edge.target_module.clone());
    }

    let all_modules: HashSet<String> = cd.module_top_names.keys().cloned().collect();
    let raw_cycles = find_cycles(&graph);
    let unique = unique_cycles(&raw_cycles);

    let star_consumed: HashSet<String> = cd.star_imported_modules.into_iter().collect();

    let skip_private: HashSet<&str> = ["__init__", "__all__", "__version__"].into_iter().collect();
    let mut dead: Vec<serde_json::Value> = Vec::new();
    for (mod_name, names) in &cd.module_top_names {
        if star_consumed.contains(mod_name) {
            continue;
        }
        let used = cd.imported_names.get(mod_name).cloned().unwrap_or_default();
        for name in names {
            if name.starts_with('_') || skip_private.contains(name.as_str()) {
                continue;
            }
            if !used.contains(name) {
                dead.push(serde_json::json!({"module": mod_name, "name": name}));
            }
        }
    }
    dead.sort_by(|a, b| {
        let am = a["module"].as_str().unwrap_or("");
        let bm = b["module"].as_str().unwrap_or("");
        let an = a["name"].as_str().unwrap_or("");
        let bn = b["name"].as_str().unwrap_or("");
        (am, an).cmp(&(bm, bn))
    });

    let mut in_degree: HashMap<String, usize> = HashMap::new();
    for edge in &cd.all_imports {
        *in_degree.entry(edge.target_module.clone()).or_insert(0) += 1;
    }

    let coupling_rows = compute_coupling(&graph, &all_modules);
    let coupling_json: Vec<serde_json::Value> = coupling_rows
        .iter()
        .map(|r| serde_json::json!({"module": r.module, "ca": r.ca, "ce": r.ce, "instability": r.instability}))
        .collect();

    let total_lines: usize = cd.file_lines.values().sum();
    let total_edges: usize = graph.values().map(|s| s.len()).sum();

    let line_total_tp = cd.test_lines + cd.prod_lines;
    let line_ratio_test = if line_total_tp > 0 {
        cd.test_lines as f64 / line_total_tp as f64
    } else {
        0.0
    };
    let fn_total_tp = cd.test_functions + cd.prod_functions;
    let fn_ratio_test = if fn_total_tp > 0 {
        cd.test_functions as f64 / fn_total_tp as f64
    } else {
        0.0
    };

    let mut complexity_rows: Vec<serde_json::Value> = cd.all_functions
        .iter()
        .map(|fn_| serde_json::json!({
            "name": fn_.qualname,
            "cc": fn_.complexity,
            "cognitive": fn_.cognitive_complexity,
            "params": fn_.param_count,
            "nesting": fn_.nesting,
            "lines": fn_.line_count,
            "file": display_rel(Path::new(&fn_.file), scan_root),
            "line": fn_.line,
            "is_method": fn_.is_method,
            "is_test": fn_.is_test,
        }))
        .collect();
    complexity_rows.sort_by(|a, b| b["cc"].as_u64().unwrap_or(0).cmp(&a["cc"].as_u64().unwrap_or(0)).then_with(|| a["name"].as_str().cmp(&b["name"].as_str())));

    let mut cognitive_rows: Vec<serde_json::Value> = cd.all_functions
        .iter()
        .map(|fn_| serde_json::json!({
            "name": fn_.qualname,
            "cognitive": fn_.cognitive_complexity,
            "file": display_rel(Path::new(&fn_.file), scan_root),
            "line": fn_.line,
            "is_method": fn_.is_method,
        }))
        .collect();
    cognitive_rows.sort_by(|a, b| {
        b["cognitive"]
            .as_u64()
            .unwrap_or(0)
            .cmp(&a["cognitive"].as_u64().unwrap_or(0))
            .then_with(|| a["name"].as_str().cmp(&b["name"].as_str()))
    });

    let code_clones = build_code_clones_py(scan_root, &cd.all_functions);
    let type1_clones = build_type1_clones_py(scan_root, &cd.all_functions);

    let mut mutable_defaults = cd.all_mutable_defaults;
    mutable_defaults.sort_by(|a, b| a.file.cmp(&b.file).then_with(|| a.line.cmp(&b.line)));

    let mut security_sorted = cd.all_security;
    security_sorted.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then_with(|| a.line.cmp(&b.line))
            .then_with(|| a.kind.cmp(&b.kind))
    });

    let mut nesting_rows: Vec<serde_json::Value> = cd.all_functions
        .iter()
        .filter(|f| f.nesting > 0)
        .map(|fn_| serde_json::json!({"name": fn_.qualname, "depth": fn_.nesting, "file": display_rel(Path::new(&fn_.file), scan_root), "line": fn_.line, "is_method": fn_.is_method}))
        .collect();
    nesting_rows.sort_by(|a, b| b["depth"].as_u64().unwrap_or(0).cmp(&a["depth"].as_u64().unwrap_or(0)).then_with(|| a["name"].as_str().cmp(&b["name"].as_str())));

    let mut silent_sorted = cd.all_silent;
    silent_sorted.sort_by(|a, b| (a.file.as_str(), a.line).cmp(&(b.file.as_str(), b.line)));

    let todo_total: usize = cd.todo_freq.values().sum();
    let mut todo_tags: Vec<_> = cd.todo_freq.iter().collect();
    todo_tags.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    let todo_by_tag: Vec<serde_json::Value> = todo_tags
        .into_iter()
        .map(|(tag, cnt)| serde_json::json!({"tag": tag, "count": cnt, "samples": cd.todo_samples.get(tag.as_str()).cloned().unwrap_or_default()}))
        .collect();

    let mut decs: Vec<_> = cd.decorator_freq.iter().collect();
    decs.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    let decorators_json: Vec<serde_json::Value> = decs
        .into_iter()
        .map(|(d, c)| serde_json::json!({"decorator": d, "count": c}))
        .collect();

    let mut routes_sorted = cd.all_routes;
    routes_sorted.sort_by(|a, b| {
        (a.path.as_str(), a.method.as_str()).cmp(&(b.path.as_str(), b.method.as_str()))
    });

    let cycles_str: Vec<String> = unique.iter().map(|c| c.join(" -> ")).collect();

    let mut files_by_lines: Vec<_> = cd.file_lines.iter().collect();
    files_by_lines.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));

    let mut largest_fn = cd.all_functions.clone();
    largest_fn.sort_by(|a, b| b.line_count.cmp(&a.line_count).then_with(|| a.qualname.cmp(&b.qualname)));

    let mut largest_cls = cd.all_classes.clone();
    largest_cls.sort_by(|a, b| b.line_count.cmp(&a.line_count).then_with(|| a.name.cmp(&b.name)));

    let mut top_imported: Vec<_> = in_degree.iter().collect();
    top_imported.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));

    serde_json::json!({
        "scanner": "python",
        "scan_root": scan_root.display().to_string(),
        "package": pkg,
        "summary": {
            "files": cd.file_count,
            "files_parsed": cd.file_count - cd.parse_errors.len(),
            "parse_errors": cd.parse_errors.len(),
            "lines": total_lines,
            "functions": cd.all_functions.len(),
            "classes": cd.all_classes.len(),
            "internal_imports": cd.all_imports.len(),
            "test_prod": {
                "test_lines": cd.test_lines,
                "production_lines": cd.prod_lines,
                "test_functions": cd.test_functions,
                "production_functions": cd.prod_functions,
                "line_ratio_test": line_ratio_test,
                "function_ratio_test": fn_ratio_test,
            },
        },
        "parse_errors": cd.parse_errors,
        "inventory": {
            "files_by_lines": files_by_lines.into_iter().map(|(f, l)| serde_json::json!({"file": f, "lines": l})).collect::<Vec<_>>(),
            "largest_functions": largest_fn.iter().map(|fn_| serde_json::json!({
                "name": fn_.qualname,
                "lines": fn_.line_count,
                "file": display_rel(Path::new(&fn_.file), scan_root),
                "line": fn_.line,
                "is_method": fn_.is_method,
            })).collect::<Vec<_>>(),
            "largest_classes": largest_cls.iter().map(|cls| serde_json::json!({
                "name": cls.name,
                "lines": cls.line_count,
                "methods": cls.methods.len(),
                "file": display_rel(Path::new(&cls.file), scan_root),
                "line": cls.line,
            })).collect::<Vec<_>>(),
        },
        "complexity": complexity_rows,
        "cognitive": cognitive_rows,
        "nesting": nesting_rows,
        "code_clones": code_clones,
        "type1_clones": type1_clones,
        "mutable_defaults": serde_json::to_value(&mutable_defaults).unwrap_or(serde_json::Value::Null),
        "security_audit": {
            "total": security_sorted.len(),
            "findings": serde_json::to_value(&security_sorted).unwrap_or(serde_json::Value::Null),
        },
        "imports": {
            "modules": all_modules.len(),
            "edges": total_edges,
            "top_imported": top_imported.into_iter().map(|(m, c)| serde_json::json!({"module": m, "count": c})).collect::<Vec<_>>(),
        },
        "coupling": coupling_json,
        "cycles": cycles_str,
        "cycles_raw": unique,
        "dead_exports": dead,
        "todo_audit": {
            "total": todo_total,
            "by_tag": todo_by_tag,
        },
        "silent_excepts": silent_sorted.iter().map(|s| serde_json::json!({"file": s.file, "line": s.line, "kind": s.kind})).collect::<Vec<_>>(),
        "decorators": decorators_json,
        "routes": routes_sorted.iter().map(|r| serde_json::json!({
            "method": r.method,
            "path": r.path,
            "handler": r.handler,
            "file": display_rel(Path::new(&r.file), scan_root),
            "line": r.line,
            "dependencies": r.dependencies,
        })).collect::<Vec<_>>(),
    })
}

pub(crate) fn analyze_python(
    scan_root: &Path,
    pkg: &str,
    exclude: &[String],
) -> anyhow::Result<serde_json::Value> {
    let scan_root = scan_root
        .canonicalize()
        .with_context(|| format!("cannot resolve {}", scan_root.display()))?;
    if !scan_root.is_dir() {
        return Err(anyhow!("Not a directory: {}", scan_root.display()));
    }

    let files = collect_py_files(&scan_root, exclude);
    let collected = collect_and_aggregate(&scan_root, pkg, &files);
    Ok(build_json(collected, &scan_root, pkg))
}
