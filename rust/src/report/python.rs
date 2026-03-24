use std::collections::HashSet;

use serde_json::Value;

use super::{
    has_parse_errors, has_todo_markers, print_code_clones_section, print_cognitive_section,
    print_counted_list, print_coupling_table, print_cycles, print_dead_exports, print_files_by_lines,
    print_import_top_modules, print_parse_errors_body, print_security_audit_section,
    print_test_prod_lines, print_todo_audit_body, section_header, sep,
};

fn section_summary(data: &Value, title: &str, skip: &HashSet<String>) {
    let s = &data["summary"];
    sep();
    println!("  {title}");
    sep();
    println!();
    println!("  Files analyzed:   {}", s["files"].as_u64().unwrap_or(0));
    if s.get("parse_errors").and_then(|v| v.as_u64()).unwrap_or(0) > 0 {
        println!(
            "  Parse errors:     {}",
            s["parse_errors"].as_u64().unwrap_or(0)
        );
    }
    println!("  Total lines:      {}", s["lines"].as_u64().unwrap_or(0));
    println!(
        "  Functions:        {}",
        s["functions"].as_u64().unwrap_or(0)
    );
    println!("  Classes:          {}", s["classes"].as_u64().unwrap_or(0));
    println!(
        "  Internal imports: {}",
        s["internal_imports"].as_u64().unwrap_or(0)
    );
    if !skip.contains("test-prod") {
        if let Some(tp) = s.get("test_prod") {
            print_test_prod_lines(tp);
        } else {
            println!();
        }
    } else {
        println!();
    }
}

fn section_inventory(data: &Value, top: usize) {
    let inv = &data["inventory"];
    section_header(&format!("1. INVENTORY — Top {top} Largest Files"));
    print_files_by_lines(inv, top);

    println!("  Top {top} Largest Functions/Methods");
    println!("  {}", "-".repeat(60));
    if let Some(arr) = inv["largest_functions"].as_array() {
        for row in arr.iter().take(top) {
            let tag = if row["is_method"].as_bool().unwrap_or(false) {
                " (method)"
            } else {
                ""
            };
            println!(
                "  {:>5} lines  {}{}  [{}:{}]",
                row["lines"].as_u64().unwrap_or(0),
                row["name"].as_str().unwrap_or(""),
                tag,
                row["file"].as_str().unwrap_or(""),
                row["line"].as_u64().unwrap_or(0)
            );
        }
    }
    println!();
    println!("  Top {top} Largest Classes");
    println!("  {}", "-".repeat(60));
    if let Some(arr) = inv["largest_classes"].as_array() {
        for row in arr.iter().take(top) {
            println!(
                "  {:>5} lines  {}  ({} methods)  [{}:{}]",
                row["lines"].as_u64().unwrap_or(0),
                row["name"].as_str().unwrap_or(""),
                row["methods"].as_u64().unwrap_or(0),
                row["file"].as_str().unwrap_or(""),
                row["line"].as_u64().unwrap_or(0)
            );
        }
    }
    println!();
}

fn section_complexity(data: &Value, cc_top: usize) {
    section_header(&format!("2. CYCLOMATIC COMPLEXITY — Top {cc_top}"));
    if let Some(arr) = data["complexity"].as_array() {
        for row in arr.iter().take(cc_top) {
            let tag = if row["is_method"].as_bool().unwrap_or(false) {
                " (method)"
            } else {
                ""
            };
            println!(
                "  CC={:>3}  {}{}  [{}:{}]",
                row["cc"].as_u64().unwrap_or(0),
                row["name"].as_str().unwrap_or(""),
                tag,
                row["file"].as_str().unwrap_or(""),
                row["line"].as_u64().unwrap_or(0)
            );
        }
    }
    println!();
}

fn section_nesting(data: &Value, top: usize) {
    if let Some(arr) = data["nesting"].as_array() {
        if !arr.is_empty() {
            let nesting_top = top.max(30);
            section_header(&format!("2b. NESTING DEPTH — Top {nesting_top}"));
            for row in arr.iter().take(nesting_top) {
                let tag = if row["is_method"].as_bool().unwrap_or(false) {
                    " (method)"
                } else {
                    ""
                };
                println!(
                    "  depth={:>2}  {}{}  [{}:{}]",
                    row["depth"].as_u64().unwrap_or(0),
                    row["name"].as_str().unwrap_or(""),
                    tag,
                    row["file"].as_str().unwrap_or(""),
                    row["line"].as_u64().unwrap_or(0)
                );
            }
            println!();
        }
    }
}

fn section_imports(data: &Value, top: usize) {
    let imp = &data["imports"];
    section_header("3. IMPORT DEPENDENCY GRAPH");
    println!(
        "  Internal modules:      {}",
        imp["modules"].as_u64().unwrap_or(0)
    );
    println!(
        "  Internal import edges: {}",
        imp["edges"].as_u64().unwrap_or(0)
    );
    println!();
    print_import_top_modules(imp, top);
}

fn section_coupling(data: &Value, top: usize) {
    if let Some(arr) = data["coupling"].as_array() {
        if !arr.is_empty() {
            section_header(&format!("3b. MODULE COUPLING — Top {top}"));
            print_coupling_table(data, top);
        }
    }
}

fn section_cycles(data: &Value) {
    section_header("4. CIRCULAR IMPORTS");
    print_cycles(data);
}

fn section_dead_exports(data: &Value, top: usize) {
    section_header("5. DEAD EXPORTS (exported but never imported internally)");
    print_dead_exports(data, top, ".");
}

fn section_todo_audit(data: &Value) {
    if has_todo_markers(data) {
        section_header("6. TODO / FIXME / HACK COMMENTS");
        print_todo_audit_body(data);
    }
}

fn section_silent_except(data: &Value) {
    section_header("7. SILENT EXCEPTION HANDLERS");
    if let Some(silent) = data["silent_excepts"].as_array() {
        if !silent.is_empty() {
            println!("  Found {} silent except handler(s):", silent.len());
            for s in silent {
                println!(
                    "    {}:{}  {}",
                    s["file"].as_str().unwrap_or(""),
                    s["line"].as_u64().unwrap_or(0),
                    s["kind"].as_str().unwrap_or("")
                );
            }
        } else {
            println!("  None found.");
        }
    }
    println!();
}

fn section_decorators(data: &Value, top: usize) {
    section_header("8. DECORATOR AUDIT");
    println!();
    println!("  Frequency Table");
    println!("  {}", "-".repeat(60));
    if let Some(arr) = data["decorators"].as_array() {
        print_counted_list(arr, top, "count", "decorator");
    }
    println!();
}

fn section_routes(data: &Value) {
    section_header("9. API ROUTE INVENTORY (FastAPI-style)");
    if let Some(routes) = data["routes"].as_array() {
        if !routes.is_empty() {
            println!("  {:<8} {:<40} {:<30} FILE", "METHOD", "PATH", "HANDLER");
            println!("  {}", "-".repeat(110));
            for r in routes {
                let rf = r["file"].as_str().unwrap_or("");
                let line = r["line"].as_u64().unwrap_or(0);
                let deps_str = r["dependencies"]
                    .as_array()
                    .filter(|d| !d.is_empty())
                    .map(|d| {
                        let joined: Vec<String> = d
                            .iter()
                            .filter_map(|x| x.as_str().map(String::from))
                            .collect();
                        format!("  deps: {}", joined.join(", "))
                    })
                    .unwrap_or_default();
                println!(
                    "  {:<8} {:<40} {:<30} {}:{}{}",
                    r["method"].as_str().unwrap_or(""),
                    r["path"].as_str().unwrap_or(""),
                    r["handler"].as_str().unwrap_or(""),
                    rf,
                    line,
                    deps_str
                );
            }
        } else {
            println!("  No routes found.");
        }
    }
    println!();
}

fn section_parse_errors(data: &Value) {
    if has_parse_errors(data) {
        section_header("PARSE ERRORS (skipped)");
        print_parse_errors_body(data, 50);
    }
}

/// Python text report.
pub(crate) fn print_python_report(data: &Value, title: &str, top: usize, skip: &HashSet<String>) {
    let cc_top = top.max(30);
    section_summary(data, title, skip);

    if !skip.contains("inventory") {
        section_inventory(data, top);
    }
    if !skip.contains("complexity") {
        section_complexity(data, cc_top);
    }
    if !skip.contains("cognitive") {
        print_cognitive_section(data, cc_top, "(Python)");
    }
    if !skip.contains("nesting") {
        section_nesting(data, top);
    }
    if !skip.contains("imports") {
        section_imports(data, top);
    }
    if !skip.contains("coupling") {
        section_coupling(data, top);
    }
    if !skip.contains("cycles") {
        section_cycles(data);
    }
    if !skip.contains("code-clones") {
        print_code_clones_section(data, top);
    }
    if !skip.contains("dead-exports") {
        section_dead_exports(data, top);
    }
    if !skip.contains("todo-audit") {
        section_todo_audit(data);
    }
    if !skip.contains("silent-except") {
        section_silent_except(data);
    }
    if !skip.contains("decorators") {
        section_decorators(data, top);
    }
    if !skip.contains("routes") {
        section_routes(data);
    }
    if !skip.contains("security-audit") {
        print_security_audit_section(data, 50);
    }
    section_parse_errors(data);

    sep();
    println!("  END OF REPORT");
    sep();
}
