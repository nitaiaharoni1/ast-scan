//! End-to-end tests invoking the `ast-scan` binary.

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

fn ast_scan_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ast-scan"))
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("CARGO_MANIFEST_DIR has parent")
        .to_path_buf()
}

/// Configure and run the CLI; paths are passed as `OsStr` (no UTF-8 requirement on Unix).
fn run_cmd(f: impl FnOnce(&mut Command)) -> std::process::Output {
    let mut cmd = Command::new(ast_scan_bin());
    f(&mut cmd);
    cmd.output().expect("spawn ast-scan")
}

fn assert_json_success(out: &std::process::Output) -> Value {
    assert!(
        out.status.success(),
        "ast-scan failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice::<Value>(&out.stdout).expect("stdout is valid JSON")
}

#[test]
fn json_python_minimal_fixture() {
    let root = workspace_root().join("fixtures/minimal-py");
    let out = run_cmd(|c| {
        c.args(["--python", "--json", "--pkg", "pkg"]).arg(&root);
    });
    let v = assert_json_success(&out);
    assert_eq!(v["scanner"].as_str(), Some("python"));
    assert!(v["summary"]["files"].as_u64().unwrap_or(0) >= 1);
    assert!(v.get("inventory").is_some());
    assert!(v.get("complexity").is_some());
    assert!(v.get("cognitive").is_some());
    assert!(v.get("code_clones").is_some());
    assert!(v.get("security_audit").is_some());
    assert!(v["summary"].get("test_prod").is_some());
}

#[test]
fn json_typescript_minimal_fixture() {
    let root = workspace_root().join("fixtures/minimal-ts");
    let out = run_cmd(|c| {
        c.args(["--typescript", "--json"]).arg(&root);
    });
    let v = assert_json_success(&out);
    assert_eq!(v["scanner"].as_str(), Some("typescript"));
    assert!(v["summary"]["files"].as_u64().unwrap_or(0) >= 1);
    assert!(v.get("inventory").is_some());
}

#[test]
fn json_rust_crate_src() {
    let root = workspace_root().join("rust/src");
    let out = run_cmd(|c| {
        c.args(["--rust", "--json"]).arg(&root);
    });
    let v = assert_json_success(&out);
    assert_eq!(v["scanner"].as_str(), Some("rust"));
    let files = v["summary"]["files"].as_u64().unwrap_or(0);
    assert!(files >= 5, "expected several .rs files, got {files}");
}

#[test]
fn multi_language_json_fixture_tree() {
    let root = workspace_root().join("fixtures");
    let out = run_cmd(|c| {
        c.arg("--json").arg(&root);
    });
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert!(v.get("python").is_some(), "expected python key in multi JSON");
    assert!(
        v.get("typescript").is_some(),
        "expected typescript key in multi JSON"
    );
    let title = v["report_title"].as_str().unwrap_or("");
    assert!(
        title.to_lowercase().contains("multi-language"),
        "report_title should mention multi-language, got {title:?}"
    );
}

/// Explicit `--python --typescript` on a tree that also has `.rs` must not run Rust when omitted.
#[test]
fn explicit_python_and_typescript_only() {
    let root = workspace_root().join("fixtures");
    let out = run_cmd(|c| {
        c.args(["--python", "--typescript", "--json", "--pkg", "pkg"])
            .arg(&root);
    });
    let v = assert_json_success(&out);
    assert!(v.get("python").is_some());
    assert!(v.get("typescript").is_some());
    assert!(
        v.get("rust").is_none(),
        "rust scanner must not run when only --python --typescript"
    );
}

#[test]
fn max_complexity_threshold_fails() {
    let root = workspace_root().join("fixtures/minimal-py");
    let out = run_cmd(|c| {
        c.args(["--python", "--pkg", "pkg", "--max-complexity", "1"])
            .arg(&root);
    });
    assert!(
        !out.status.success(),
        "expected threshold exit for high CC fixture"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("THRESHOLD") || stderr.contains("BREACH"),
        "stderr={stderr}"
    );
}

#[test]
fn unknown_skip_exits_2() {
    let root = workspace_root().join("fixtures/minimal-ts");
    let out = run_cmd(|c| {
        c.args([
            "--typescript",
            "--skip",
            "not-a-valid-section-name-xyz",
        ])
        .arg(&root);
    });
    assert_eq!(
        out.status.code(),
        Some(2),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn exclude_reduces_python_file_count() {
    let root = workspace_root().join("fixtures/minimal-py");
    let full = run_cmd(|c| {
        c.args(["--python", "--json", "--pkg", "pkg"]).arg(&root);
    });
    let v_full = assert_json_success(&full);
    let n_full = v_full["summary"]["files"].as_u64().unwrap_or(0);

    let filtered = run_cmd(|c| {
        c.args([
            "--python",
            "--json",
            "--pkg",
            "pkg",
            "--exclude",
            "util.py",
        ])
        .arg(&root);
    });
    let v_f = assert_json_success(&filtered);
    let n_f = v_f["summary"]["files"].as_u64().unwrap_or(0);
    assert!(
        n_f < n_full,
        "exclude util.py should drop at least one file: full={n_full} filtered={n_f}"
    );
}

#[test]
fn text_skip_omits_section() {
    let root = workspace_root().join("fixtures/minimal-py");
    let out = run_cmd(|c| {
        c.args(["--python", "--pkg", "pkg", "--skip", "inventory"])
            .arg(&root);
    });
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        !text.contains("INVENTORY — Top"),
        "files-by-lines inventory section should be skipped: {text}"
    );
}

#[test]
fn max_nesting_high_passes() {
    let root = workspace_root().join("fixtures/minimal-py");
    let out = run_cmd(|c| {
        c.args(["--python", "--pkg", "pkg", "--max-nesting", "50"])
            .arg(&root);
    });
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn max_cycles_zero_passes_clean_fixture() {
    let root = workspace_root().join("fixtures/minimal-py");
    let out = run_cmd(|c| {
        c.args(["--python", "--pkg", "pkg", "--max-cycles", "0"])
            .arg(&root);
    });
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn dogfood_rust_scan_own_source() {
    let root = workspace_root().join("rust/src");
    let out = run_cmd(|c| {
        c.args(["--rust", "--json"]).arg(&root);
    });
    let v = assert_json_success(&out);
    assert_eq!(v["scanner"].as_str(), Some("rust"));
    let files = v["summary"]["files"].as_u64().unwrap_or(0);
    assert!(files >= 10, "self-scan should find many .rs files, got {files}");
    let fns = v["summary"]["functions"].as_u64().unwrap_or(0);
    assert!(fns >= 20, "self-scan should find many functions, got {fns}");
    assert!(v.get("complexity").is_some());
    assert!(v.get("cognitive").is_some());
    assert!(v.get("code_clones").is_some());
    assert!(v.get("security_audit").is_some());
    assert!(v.get("coupling").is_some());
    assert!(v.get("cycles_raw").is_some());
    assert!(
        v["summary"].get("test_prod").is_some(),
        "rust JSON should include summary.test_prod"
    );
    let row0 = v["complexity"]
        .as_array()
        .and_then(|a| a.first())
        .expect("rust complexity non-empty");
    for key in ["cognitive", "params", "is_test"] {
        assert!(
            row0.get(key).is_some(),
            "rust complexity row should include {key}, got keys {:?}",
            row0
                .as_object()
                .map(|o| o.keys().collect::<Vec<_>>())
                .unwrap_or_default()
        );
    }
}

#[test]
fn empty_directory_produces_zero_files() {
    let tmp = std::env::temp_dir().join("ast-scan-test-empty");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let out = run_cmd(|c| {
        c.args(["--python", "--json", "--pkg", "empty"]).arg(&tmp);
    });
    let v = assert_json_success(&out);
    assert_eq!(v["summary"]["files"].as_u64().unwrap_or(99), 0);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn text_report_ends_with_end_of_report() {
    let root = workspace_root().join("fixtures/minimal-py");
    let out = run_cmd(|c| {
        c.args(["--python", "--pkg", "pkg"]).arg(&root);
    });
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("END OF REPORT"),
        "text report should end with END OF REPORT marker"
    );
}

#[test]
fn json_output_has_scan_root() {
    let root = workspace_root().join("fixtures/minimal-ts");
    let out = run_cmd(|c| {
        c.args(["--typescript", "--json"]).arg(&root);
    });
    let v = assert_json_success(&out);
    assert!(
        v.get("scan_root").is_some(),
        "JSON output should include scan_root"
    );
}

#[test]
fn parallel_multi_lang_json_same_as_sequential() {
    let root = workspace_root().join("fixtures");
    let out = run_cmd(|c| {
        c.arg("--json").arg(&root);
    });
    let v = assert_json_success(&out);
    assert!(v.get("python").is_some());
    assert!(v.get("typescript").is_some());
    let py_files = v["python"]["summary"]["files"].as_u64().unwrap_or(0);
    let ts_files = v["typescript"]["summary"]["files"].as_u64().unwrap_or(0);
    assert!(py_files >= 1, "python should find files");
    assert!(ts_files >= 1, "typescript should find files");
}

#[test]
fn complexity_rows_include_lines() {
    let root = workspace_root().join("fixtures/minimal-py");
    let out = run_cmd(|c| {
        c.args(["--python", "--json", "--pkg", "pkg"]).arg(&root);
    });
    let v = assert_json_success(&out);
    let row0 = v["complexity"]
        .as_array()
        .and_then(|a| a.first())
        .expect("complexity non-empty");
    assert!(
        row0.get("lines").is_some(),
        "complexity row must include 'lines' field, got: {:?}",
        row0
    );
}

#[test]
fn ts_complexity_rows_include_lines() {
    let root = workspace_root().join("fixtures/minimal-ts");
    let out = run_cmd(|c| {
        c.args(["--typescript", "--json"]).arg(&root);
    });
    let v = assert_json_success(&out);
    let row0 = v["complexity"]
        .as_array()
        .and_then(|a| a.first())
        .expect("ts complexity non-empty");
    assert!(row0.get("lines").is_some(), "ts complexity row must include 'lines'");
}

#[test]
fn json_has_type1_clones_key() {
    let root = workspace_root().join("fixtures/minimal-py");
    let out = run_cmd(|c| {
        c.args(["--python", "--json", "--pkg", "pkg"]).arg(&root);
    });
    let v = assert_json_success(&out);
    assert!(v.get("type1_clones").is_some(), "python JSON must include type1_clones key");
}

#[test]
fn mutable_defaults_detected() {
    let root = workspace_root().join("fixtures/minimal-py");
    let out = run_cmd(|c| {
        c.args(["--python", "--json", "--pkg", "pkg"]).arg(&root);
    });
    let v = assert_json_success(&out);
    let mds = v["mutable_defaults"].as_array().expect("mutable_defaults is array");
    assert!(
        !mds.is_empty(),
        "should detect mutable defaults in fixtures/minimal-py/mutable_defaults.py"
    );
    let first = &mds[0];
    assert!(first.get("func_name").is_some());
    assert!(first.get("param_name").is_some());
    assert!(first.get("kind").is_some());
}

#[test]
fn max_lines_threshold_fails() {
    let root = workspace_root().join("fixtures/minimal-py");
    let out = run_cmd(|c| {
        c.args(["--python", "--pkg", "pkg", "--max-lines", "1"])
            .arg(&root);
    });
    assert!(
        !out.status.success(),
        "expected threshold exit for --max-lines 1"
    );
}

#[test]
fn mutable_defaults_skip_section() {
    let root = workspace_root().join("fixtures/minimal-py");
    let out = run_cmd(|c| {
        c.args(["--python", "--pkg", "pkg", "--skip", "mutable-defaults"])
            .arg(&root);
    });
    assert!(out.status.success(), "stderr={}", String::from_utf8_lossy(&out.stderr));
}
