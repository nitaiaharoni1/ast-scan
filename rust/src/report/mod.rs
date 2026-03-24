//! Text reports from analysis JSON (`serde_json::Value`).

mod python;
mod rust_report;
mod typescript;

pub(crate) use python::print_python_report;
pub(crate) use rust_report::print_rust_report;
pub(crate) use typescript::print_ts_report;

use serde_json::Value;

fn sep() {
    println!("{}", "=".repeat(72));
}

fn fmt_grouped_u64(n: u64) -> String {
    let rev: String = n.to_string().chars().rev().collect();
    let mut acc = String::new();
    for (i, c) in rev.chars().enumerate() {
        if i > 0 && i % 3 == 0 {
            acc.push(',');
        }
        acc.push(c);
    }
    acc.chars().rev().collect()
}

fn section_header(label: &str) {
    sep();
    println!("  {label}");
    sep();
}

fn sub_header(label: &str) {
    println!("  {label}");
    println!("  {}", "-".repeat(60));
}

fn print_files_by_lines(inv: &Value, top: usize) {
    if let Some(arr) = inv["files_by_lines"].as_array() {
        for row in arr.iter().take(top) {
            println!(
                "  {:>5} lines  {}",
                row["lines"].as_u64().unwrap_or(0),
                row["file"].as_str().unwrap_or("")
            );
        }
    }
    println!();
}

fn print_counted_list(arr: &[Value], top: usize, count_key: &str, label_key: &str) {
    for row in arr.iter().take(top) {
        println!(
            "  {:>4}x  {}",
            row[count_key].as_u64().unwrap_or(0),
            row[label_key].as_str().unwrap_or("")
        );
    }
}

fn print_counted_list_with_files(
    arr: &[Value],
    count_key: &str,
    label_key: &str,
    files_key: &str,
) {
    for r in arr {
        println!(
            "  {:>4}x  {}",
            r[count_key].as_u64().unwrap_or(0),
            r[label_key].as_str().unwrap_or("")
        );
        if let Some(files) = r[files_key].as_array() {
            let joined: Vec<&str> = files.iter().filter_map(|v| v.as_str()).collect();
            if !joined.is_empty() {
                println!("         {}", joined.join("; "));
            }
        }
    }
}

fn print_coupling_table(data: &Value, top: usize) {
    if let Some(arr) = data["coupling"].as_array() {
        if !arr.is_empty() {
            println!("  {:<40} {:>4} {:>4} {:>5}", "MODULE", "Ca", "Ce", "I");
            println!("  {}", "-".repeat(60));
            for row in arr.iter().take(top) {
                println!(
                    "  {:<40} {:>4} {:>4} {:>5.2}",
                    row["module"].as_str().unwrap_or(""),
                    row["ca"].as_u64().unwrap_or(0),
                    row["ce"].as_u64().unwrap_or(0),
                    row["instability"].as_f64().unwrap_or(0.0)
                );
            }
            println!();
        }
    }
}

fn print_cycles(data: &Value) {
    if let Some(raw) = data["cycles_raw"].as_array() {
        if !raw.is_empty() {
            println!("  Found {} cycle(s):", raw.len());
            for (i, c) in raw.iter().enumerate() {
                let parts: Vec<&str> = c
                    .as_array()
                    .map(|a| a.iter().filter_map(|x| x.as_str()).collect())
                    .unwrap_or_default();
                println!("  [{}] {}", i + 1, parts.join(" -> "));
            }
        } else {
            println!("  None found.");
        }
    }
    println!();
}

fn print_dead_exports(data: &Value, top: usize, joiner: &str) {
    if let Some(dead) = data["dead_exports"].as_array() {
        if !dead.is_empty() {
            let cap = 80.max(top * 4);
            println!("  Found {} potentially dead export(s):", dead.len());
            for d in dead.iter().take(cap) {
                println!(
                    "    {}{}{}",
                    d["module"].as_str().unwrap_or(""),
                    joiner,
                    d["name"].as_str().unwrap_or("")
                );
            }
            if dead.len() > cap {
                println!("    ... and {} more", dead.len() - cap);
            }
        } else {
            println!("  None found.");
        }
    }
    println!();
}

fn print_todo_audit_body(data: &Value) {
    let todos = &data["todo_audit"];
    let total = todos["total"].as_u64().unwrap_or(0);
    println!("  {total} marker(s) found:");
    println!();
    if let Some(arr) = todos["by_tag"].as_array() {
        print_counted_list_with_files(arr, "count", "tag", "samples");
    }
    println!();
}

fn has_todo_markers(data: &Value) -> bool {
    data["todo_audit"]["total"].as_u64().unwrap_or(0) > 0
}

fn print_parse_errors_body(data: &Value, cap: usize) {
    if let Some(pe) = data.get("parse_errors").and_then(|v| v.as_array()) {
        if !pe.is_empty() {
            for e in pe.iter().take(cap) {
                println!(
                    "    {}: {}",
                    e["file"].as_str().unwrap_or(""),
                    e["message"].as_str().unwrap_or("")
                );
            }
            if pe.len() > cap {
                println!("    ... and {} more", pe.len() - cap);
            }
            println!();
        }
    }
}

fn has_parse_errors(data: &Value) -> bool {
    data.get("parse_errors")
        .and_then(|v| v.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false)
}

fn print_import_top_modules(imp: &Value, top: usize) {
    sub_header(&format!("Top {top} Most-Imported Internal Modules"));
    if let Some(arr) = imp["top_imported"].as_array() {
        print_counted_list(arr, top, "count", "module");
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fmt_grouped_u64_small() {
        assert_eq!(fmt_grouped_u64(0), "0");
        assert_eq!(fmt_grouped_u64(123), "123");
        assert_eq!(fmt_grouped_u64(999), "999");
    }

    #[test]
    fn test_fmt_grouped_u64_thousands() {
        assert_eq!(fmt_grouped_u64(1_000), "1,000");
        assert_eq!(fmt_grouped_u64(12_345), "12,345");
        assert_eq!(fmt_grouped_u64(1_234_567), "1,234,567");
    }

    #[test]
    fn test_has_todo_markers_true() {
        let data = serde_json::json!({"todo_audit": {"total": 3}});
        assert!(has_todo_markers(&data));
    }

    #[test]
    fn test_has_todo_markers_false() {
        let data = serde_json::json!({"todo_audit": {"total": 0}});
        assert!(!has_todo_markers(&data));
    }

    #[test]
    fn test_has_parse_errors_empty() {
        let data = serde_json::json!({"parse_errors": []});
        assert!(!has_parse_errors(&data));
    }

    #[test]
    fn test_has_parse_errors_with_data() {
        let data = serde_json::json!({"parse_errors": [{"file": "x.rs", "message": "err"}]});
        assert!(has_parse_errors(&data));
    }

    #[test]
    fn test_has_parse_errors_missing() {
        let data = serde_json::json!({});
        assert!(!has_parse_errors(&data));
    }
}
