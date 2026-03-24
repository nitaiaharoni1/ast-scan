use std::collections::HashSet;

use serde_json::Value;

use super::{
    fmt_grouped_u64, has_parse_errors, has_todo_markers, print_counted_list,
    print_coupling_table, print_cycles, print_dead_exports, print_files_by_lines,
    print_import_top_modules, print_parse_errors_body, print_todo_audit_body, section_header,
    sep, sub_header,
};

fn section_summary(data: &Value, title: &str) {
    let s = &data["summary"];
    sep();
    println!("  {title}");
    sep();
    println!();
    println!(
        "  Files analyzed:      {}",
        s["files"].as_u64().unwrap_or(0)
    );
    println!(
        "  Total lines:         {}",
        fmt_grouped_u64(s["lines"].as_u64().unwrap_or(0))
    );
    println!(
        "  Functions:           {}",
        s["functions"].as_u64().unwrap_or(0)
    );
    println!(
        "  Structs / enums:     {}",
        s["structs_enums"].as_u64().unwrap_or(0)
    );
    println!(
        "  Traits:              {}",
        s["traits"].as_u64().unwrap_or(0)
    );
    println!(
        "  Internal imports:    {}",
        s["internal_imports"].as_u64().unwrap_or(0)
    );
    println!(
        "  External imports:    {}",
        s["external_imports"].as_u64().unwrap_or(0)
    );
    if s.get("parse_errors").and_then(|v| v.as_u64()).unwrap_or(0) > 0 {
        println!(
            "  Parse errors:        {}",
            s["parse_errors"].as_u64().unwrap_or(0)
        );
    }
    println!();
}

fn section_inventory(data: &Value, top: usize) {
    let inv = &data["inventory"];
    section_header(&format!("1. INVENTORY — Top {top} Largest Files"));
    print_files_by_lines(inv, top);

    println!("  Top {top} Largest Functions");
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
    if let Some(arr) = inv["largest_types"].as_array() {
        if !arr.is_empty() {
            println!("  Top {top} Largest Structs / Enums");
            println!("  {}", "-".repeat(60));
            for row in arr.iter().take(top) {
                println!(
                    "  {:>5} lines  {} ({})  {} methods  [{}:{}]",
                    row["lines"].as_u64().unwrap_or(0),
                    row["name"].as_str().unwrap_or(""),
                    row["kind"].as_str().unwrap_or(""),
                    row["methods"].as_u64().unwrap_or(0),
                    row["file"].as_str().unwrap_or(""),
                    row["line"].as_u64().unwrap_or(0)
                );
            }
            println!();
        }
    }
}

fn section_complexity(data: &Value, cc_top: usize) {
    section_header(&format!("2. CYCLOMATIC COMPLEXITY — Top {cc_top}"));
    if let Some(arr) = data["complexity"].as_array() {
        for row in arr.iter().take(cc_top) {
            let u = if row["is_unsafe"].as_bool().unwrap_or(false) {
                " unsafe"
            } else {
                ""
            };
            let tag = if row["is_method"].as_bool().unwrap_or(false) {
                " (method)"
            } else {
                ""
            };
            println!(
                "  CC={:>3}  {}{}{}  [{}:{}]",
                row["cc"].as_u64().unwrap_or(0),
                row["name"].as_str().unwrap_or(""),
                tag,
                u,
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
            let nest_top = top.max(30);
            section_header(&format!("2b. NESTING DEPTH — Top {nest_top}"));
            for row in arr.iter().take(nest_top) {
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
    section_header("3. IMPORT DEPENDENCY GRAPH (crate-internal)");
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
    let ext_top = 15.min(top);
    sub_header(&format!("Top {ext_top} External Crates"));
    if let Some(arr) = imp["external_crates"].as_array() {
        print_counted_list(arr, ext_top, "count", "crate");
    }
    println!();
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
    section_header("5. DEAD EXPORTS (pub, never imported internally by name)");
    print_dead_exports(data, top, " :: ");
}

fn section_unsafe_audit(data: &Value, top: usize) {
    let ua = &data["unsafe_audit"];
    section_header("6. UNSAFE AUDIT");
    println!(
        "  Unsafe functions: {}",
        ua["unsafe_functions"].as_u64().unwrap_or(0)
    );
    println!(
        "  Unsafe blocks:    {}",
        ua["unsafe_blocks"].as_u64().unwrap_or(0)
    );
    println!();
    if let Some(arr) = ua["by_file"].as_array() {
        for row in arr.iter().take(top.max(30)) {
            println!(
                "  {}  blocks={}  unsafe_fn={}",
                row["file"].as_str().unwrap_or(""),
                row["unsafe_blocks"].as_u64().unwrap_or(0),
                row["unsafe_functions"].as_u64().unwrap_or(0)
            );
        }
    }
    println!();
}

fn section_unwrap_audit(data: &Value, top: usize) {
    let ua = &data["unwrap_audit"];
    section_header("7. UNWRAP / EXPECT AUDIT");
    println!(
        "  Total .unwrap() / .expect() sites: {}",
        ua["total"].as_u64().unwrap_or(0)
    );
    println!();
    if let Some(arr) = ua["by_file"].as_array() {
        print_counted_list(arr, top.max(30), "count", "file");
    }
    println!();
}

fn section_allow_lints(data: &Value, top: usize) {
    let al = &data["allow_lints"];
    section_header("8. #[allow(...)] LINT AUDIT");
    if al["total"].as_u64().unwrap_or(0) > 0 {
        println!(
            "  {} allow attribute(s):",
            al["total"].as_u64().unwrap_or(0)
        );
        println!();
        if let Some(arr) = al["by_rule"].as_array() {
            print_counted_list(arr, top.max(40), "count", "rule");
        }
    } else {
        println!("  No #[allow(...)] attributes found.");
    }
    println!();
}

fn section_derive_audit(data: &Value, top: usize) {
    let da = &data["derive_audit"];
    section_header("9. DERIVE MACRO AUDIT");
    if da["total"].as_u64().unwrap_or(0) > 0 {
        println!("  {} derive use(s):", da["total"].as_u64().unwrap_or(0));
        println!();
        if let Some(arr) = da["by_derive"].as_array() {
            print_counted_list(arr, top.max(40), "count", "derive");
        }
    } else {
        println!("  No derive macros found.");
    }
    println!();
}

fn section_traits(data: &Value, top: usize) {
    if let Some(arr) = data["traits_inventory"].as_array() {
        if !arr.is_empty() {
            section_header("10. TRAIT INVENTORY");
            for row in arr.iter().take(top.max(50)) {
                println!(
                    "  {}  [{}:{}]  {}",
                    row["name"].as_str().unwrap_or(""),
                    row["file"].as_str().unwrap_or(""),
                    row["line"].as_u64().unwrap_or(0),
                    row["visibility"].as_str().unwrap_or("")
                );
            }
            println!();
        }
    }
}

fn section_todo_audit(data: &Value) {
    if has_todo_markers(data) {
        section_header("11. TODO / FIXME / HACK COMMENTS");
        print_todo_audit_body(data);
    }
}

fn section_parse_errors(data: &Value) {
    if has_parse_errors(data) {
        section_header("PARSE ERRORS (skipped files)");
        print_parse_errors_body(data, 50);
    }
}

/// Rust crate text report.
pub(crate) fn print_rust_report(data: &Value, title: &str, top: usize, skip: &HashSet<String>) {
    let cc_top = top.max(30);
    section_summary(data, title);

    if !skip.contains("inventory") {
        section_inventory(data, top);
    }
    if !skip.contains("complexity") {
        section_complexity(data, cc_top);
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
    if !skip.contains("dead-exports") {
        section_dead_exports(data, top);
    }
    if !skip.contains("unsafe-audit") {
        section_unsafe_audit(data, top);
    }
    if !skip.contains("unwrap-audit") {
        section_unwrap_audit(data, top);
    }
    if !skip.contains("allow-lints") {
        section_allow_lints(data, top);
    }
    if !skip.contains("derive-audit") {
        section_derive_audit(data, top);
    }
    if !skip.contains("traits") {
        section_traits(data, top);
    }
    if !skip.contains("todo-audit") {
        section_todo_audit(data);
    }
    if !skip.contains("parse-errors") {
        section_parse_errors(data);
    }

    sep();
    println!("  END OF REPORT");
    sep();
}
