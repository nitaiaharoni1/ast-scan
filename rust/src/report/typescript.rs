use std::collections::HashSet;

use serde_json::Value;

use super::{
    fmt_grouped_u64, has_todo_markers, print_code_clones_section, print_cognitive_section,
    print_counted_list, print_counted_list_with_files, print_coupling_table, print_cycles,
    print_dead_exports, print_files_by_lines, print_import_top_modules, print_security_audit_section,
    print_test_prod_lines, print_todo_audit_body, print_type1_clones_section, section_header, sep,
    sub_header,
};

fn section_summary(data: &Value, title: &str, skip: &HashSet<String>) {
    let s = &data["summary"];
    sep();
    println!("  {title}");
    sep();
    println!();
    println!("  Files analyzed:    {}", s["files"].as_u64().unwrap_or(0));
    println!(
        "  Total lines:       {}",
        fmt_grouped_u64(s["lines"].as_u64().unwrap_or(0))
    );
    println!(
        "  Functions/consts:  {}",
        s["functions"].as_u64().unwrap_or(0)
    );
    println!(
        "  Classes:           {}",
        s["classes"].as_u64().unwrap_or(0)
    );
    println!(
        "  React components:  {}",
        s["components"].as_u64().unwrap_or(0)
    );
    println!(
        "  Custom hooks:      {}",
        s["custom_hooks"].as_u64().unwrap_or(0)
    );
    println!(
        "  Internal imports:  {}",
        s["internal_imports"].as_u64().unwrap_or(0)
    );
    println!(
        "  External imports:  {}",
        s["external_imports"].as_u64().unwrap_or(0)
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

    println!("  Top {top} Largest Functions");
    println!("  {}", "-".repeat(60));
    if let Some(arr) = inv["largest_functions"].as_array() {
        for row in arr.iter().take(top) {
            let tag = if row["is_component"].as_bool().unwrap_or(false) {
                " [component]"
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
    if let Some(arr) = inv["largest_classes"].as_array() {
        if !arr.is_empty() {
            println!("  Top {top} Largest Classes");
            println!("  {}", "-".repeat(60));
            for row in arr.iter().take(top) {
                let heritage = if row["has_heritage"].as_bool().unwrap_or(false) {
                    " (extends)"
                } else {
                    ""
                };
                println!(
                    "  {:>5} lines  {}  ({} methods, {} props){}  [{}:{}]",
                    row["lines"].as_u64().unwrap_or(0),
                    row["name"].as_str().unwrap_or(""),
                    row["methods"].as_u64().unwrap_or(0),
                    row["properties"].as_u64().unwrap_or(0),
                    heritage,
                    row["file"].as_str().unwrap_or(""),
                    row["line"].as_u64().unwrap_or(0)
                );
            }
            println!();
        }
    }
}

fn ts_row_tag(row: &Value) -> &'static str {
    if row["is_component"].as_bool().unwrap_or(false) {
        " [component]"
    } else {
        ""
    }
}

fn section_complexity(data: &Value, cc_top: usize) {
    section_header(&format!("2. CYCLOMATIC COMPLEXITY — Top {cc_top}"));
    if let Some(arr) = data["complexity"].as_array() {
        for row in arr.iter().take(cc_top) {
            println!(
                "  CC={:>3}  {}{}  [{}:{}]",
                row["cc"].as_u64().unwrap_or(0),
                row["name"].as_str().unwrap_or(""),
                ts_row_tag(row),
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
                println!(
                    "  depth={:>2}  {}{}  [{}:{}]",
                    row["depth"].as_u64().unwrap_or(0),
                    row["name"].as_str().unwrap_or(""),
                    ts_row_tag(row),
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
    let ext = imp["external_packages"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    section_header("3. IMPORT DEPENDENCY GRAPH");
    println!(
        "  Internal modules:      {}",
        imp["modules"].as_u64().unwrap_or(0)
    );
    println!(
        "  Internal import edges: {}",
        imp["edges"].as_u64().unwrap_or(0)
    );
    println!("  External packages:     {ext}");
    println!();
    print_import_top_modules(imp, top);
    let ext_top = 15.min(top);
    sub_header(&format!("Top {ext_top} External Packages"));
    if let Some(arr) = imp["external_packages"].as_array() {
        print_counted_list(arr, ext_top, "count", "package");
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
    section_header("5. DEAD EXPORTS (exported but never imported internally)");
    print_dead_exports(data, top, " :: ");
}

fn section_component_props(data: &Value) {
    let props = data["component_props"].as_array();
    let comp_total = data["summary"]["components"].as_u64().unwrap_or(0);
    let props_len = props.map(|a| a.len()).unwrap_or(0);
    section_header("6. COMPONENT PROPS");
    println!("  {comp_total} components total, {props_len} with extractable props");
    println!();
    if let Some(arr) = props {
        for comp in arr {
            println!(
                "  {}  [{}:{}]",
                comp["name"].as_str().unwrap_or(""),
                comp["file"].as_str().unwrap_or(""),
                comp["line"].as_u64().unwrap_or(0)
            );
            let plist: Vec<&str> = comp["props"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            println!("    props: {}", plist.join(", "));
        }
    }
    println!();
}

fn section_hooks(data: &Value, top: usize) {
    let hooks = &data["hooks"];
    section_header("7. HOOK USAGE PATTERNS");
    println!();
    sub_header("Hook Frequency (across all components)");
    if let Some(arr) = hooks["frequency"].as_array() {
        for row in arr.iter().take(top) {
            let tag = if row["is_custom"].as_bool().unwrap_or(false) {
                " [custom]"
            } else {
                ""
            };
            println!(
                "  {:>4}x  {}{}",
                row["count"].as_u64().unwrap_or(0),
                row["hook"].as_str().unwrap_or(""),
                tag
            );
        }
    }
    println!();
    sub_header("Custom Hooks Inventory");
    if let Some(arr) = hooks["custom_hooks_inventory"].as_array() {
        for fn_ in arr {
            println!(
                "  {}  ({} lines)  [{}:{}]",
                fn_["name"].as_str().unwrap_or(""),
                fn_["lines"].as_u64().unwrap_or(0),
                fn_["file"].as_str().unwrap_or(""),
                fn_["line"].as_u64().unwrap_or(0)
            );
        }
    }
    println!();
    sub_header("Per-Component Hook Usage (components using 3+ hooks)");
    if let Some(arr) = hooks["heavy_components"].as_array() {
        for comp in arr.iter().take(top) {
            println!(
                "  {}  [{}:{}]",
                comp["name"].as_str().unwrap_or(""),
                comp["file"].as_str().unwrap_or(""),
                comp["line"].as_u64().unwrap_or(0)
            );
            let hlist: Vec<&str> = comp["hooks"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            println!(
                "    hooks ({}): {}",
                comp["hook_count"].as_u64().unwrap_or(0),
                hlist.join(", ")
            );
        }
    }
    println!();
}

fn section_console_debugger(data: &Value) {
    let cd = &data["console_debugger"];
    let total = cd["total"].as_u64().unwrap_or(0);
    if total > 0 {
        section_header("7b. CONSOLE / DEBUGGER STATEMENTS");
        println!("  {total} statement(s) found:");
        println!();
        if let Some(arr) = cd["by_kind"].as_array() {
            print_counted_list(arr, arr.len(), "count", "kind");
        }
        println!();
    }
}

fn section_silent_catches(data: &Value) {
    section_header("8. SILENT CATCHES");
    if let Some(catches) = data["silent_catches"].as_array() {
        if !catches.is_empty() {
            println!("  Found {} empty/trivial catch handler(s):", catches.len());
            for c in catches {
                println!(
                    "    {}:{}  {}",
                    c["file"].as_str().unwrap_or(""),
                    c["line"].as_u64().unwrap_or(0),
                    c["kind"].as_str().unwrap_or("")
                );
            }
        } else {
            println!("  None found.");
        }
    }
    println!();
}

fn section_eslint_disables(data: &Value) {
    let disables = &data["eslint_disables"];
    section_header("9. ESLINT DISABLE AUDIT");
    let dtotal = disables["total"].as_u64().unwrap_or(0);
    if dtotal > 0 {
        let nrules = disables["unique_rules"].as_u64().unwrap_or(0);
        println!("  {dtotal} disable(s) across {nrules} rule(s):");
        println!();
        if let Some(arr) = disables["by_rule"].as_array() {
            print_counted_list_with_files(arr, "count", "rule", "files");
        }
    } else {
        println!("  No eslint-disable comments found.");
    }
    println!();
}

fn section_any_audit(data: &Value, top: usize) {
    let any_data = &data["any_audit"];
    section_header("9b. EXPLICIT `any` TYPE AUDIT");
    let atotal = any_data["total"].as_u64().unwrap_or(0);
    if atotal > 0 {
        let by_file = any_data["by_file"].as_array();
        let nfiles = by_file.map(|a| a.len()).unwrap_or(0);
        println!("  {atotal} explicit `any` type(s) across {nfiles} file(s):");
        println!();
        if let Some(arr) = by_file {
            print_counted_list(arr, top, "count", "file");
            if arr.len() > top {
                println!("    ... and {} more files", arr.len() - top);
            }
        }
    } else {
        println!("  No explicit `any` types found.");
    }
    println!();
}

fn section_ts_directives(data: &Value) {
    let dirs = &data["ts_directives"];
    section_header("10. TS DIRECTIVE AUDIT (@ts-ignore / @ts-expect-error)");
    let dtot = dirs["total"].as_u64().unwrap_or(0);
    if dtot > 0 {
        println!("  {dtot} directive(s):");
        println!();
        if let Some(arr) = dirs["by_directive"].as_array() {
            print_counted_list_with_files(arr, "count", "directive", "files");
        }
    } else {
        println!("  No @ts-ignore/@ts-expect-error/@ts-nocheck directives found.");
    }
    println!();
}

fn section_todo_audit(data: &Value) {
    if has_todo_markers(data) {
        section_header("11. TODO / FIXME / HACK COMMENTS");
        print_todo_audit_body(data);
    }
}

fn section_mobx_observer(data: &Value) {
    if let Some(mobx) = data["mobx_observer"].as_array() {
        if !mobx.is_empty() {
            section_header("12. MOBX OBSERVER — Unwrapped Components");
            println!(
                "  Found {} component(s) not wrapped in observer():",
                mobx.len()
            );
            for m in mobx {
                println!(
                    "    {}:{}  {}  ({})",
                    m["file"].as_str().unwrap_or(""),
                    m["line"].as_u64().unwrap_or(0),
                    m["component"].as_str().unwrap_or(""),
                    m["kind"].as_str().unwrap_or("")
                );
            }
            println!();
        }
    }
}

fn section_orm_case_check(data: &Value) {
    if let Some(orm) = data["orm_case_check"].as_object() {
        if let Some(findings) = orm["findings"].as_array() {
            if !findings.is_empty() {
                section_header("ORM CASE CONVENTION CHECK");
                let methods: Vec<&str> = orm["methods"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();
                println!("  Methods checked: {}", methods.join(", "));
                println!(
                    "  Found {} possible camelCase identifier(s) in query strings:",
                    findings.len()
                );
                println!();
                for f in findings {
                    println!(
                        "    {}:{}  .{}()  {}",
                        f["file"].as_str().unwrap_or(""),
                        f["line"].as_u64().unwrap_or(0),
                        f["method"].as_str().unwrap_or(""),
                        f["snippet"].as_str().unwrap_or("")
                    );
                }
                println!();
            }
        }
    }
}

fn section_import_boundaries(data: &Value) -> bool {
    if let Some(obj) = data["import_boundaries"].as_object() {
        if let Some(violations) = obj["violations"].as_array() {
            if !violations.is_empty() {
                section_header("IMPORT BOUNDARY VIOLATIONS");
                println!("  {} violation(s) found:", violations.len());
                println!();
                for v in violations {
                    println!(
                        "    {}:{}  imports {}  ({})",
                        v["file"].as_str().unwrap_or(""),
                        v["line"].as_u64().unwrap_or(0),
                        v["import_source"].as_str().unwrap_or(""),
                        v["rule"].as_str().unwrap_or("")
                    );
                }
                println!();
                return true;
            }
        }
    }
    false
}

/// TypeScript/JavaScript text report.
/// Returns `true` if import boundary violations were reported (caller should `exit(1)`).
pub(crate) fn print_ts_report(data: &Value, title: &str, top: usize, skip: &HashSet<String>) -> bool {
    let cc_top = top.max(30);
    section_summary(data, title, skip);

    if !skip.contains("inventory") {
        section_inventory(data, top);
    }
    if !skip.contains("complexity") {
        section_complexity(data, cc_top);
    }
    if !skip.contains("cognitive") {
        print_cognitive_section(data, cc_top, "(TypeScript)");
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
        print_type1_clones_section(data, top);
        print_code_clones_section(data, top);
    }
    if !skip.contains("dead-exports") {
        section_dead_exports(data, top);
    }
    if !skip.contains("component-props") {
        section_component_props(data);
    }
    if !skip.contains("hooks") {
        section_hooks(data, top);
    }
    if !skip.contains("console-debugger") {
        section_console_debugger(data);
    }
    if !skip.contains("silent-catches") {
        section_silent_catches(data);
    }
    if !skip.contains("eslint-disables") {
        section_eslint_disables(data);
    }
    if !skip.contains("any-audit") {
        section_any_audit(data, top);
    }
    if !skip.contains("ts-directives") {
        section_ts_directives(data);
    }
    if !skip.contains("todo-audit") {
        section_todo_audit(data);
    }
    if !skip.contains("mobx-observer") {
        section_mobx_observer(data);
    }
    if !skip.contains("orm-case-check") {
        section_orm_case_check(data);
    }
    if !skip.contains("security-audit") {
        print_security_audit_section(data, 50);
    }

    let mut boundary_fail = false;
    if !skip.contains("import-boundaries") {
        boundary_fail = section_import_boundaries(data);
    }

    sep();
    println!("  END OF REPORT");
    sep();

    boundary_fail
}
