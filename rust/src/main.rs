mod audits;
mod graph;
mod python_scanner;
mod report;
mod rust_scanner;
mod scanner;
mod ts_checks;
mod ts_scanner;
mod types;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context};
use clap::Parser;
use serde_json::{Map, Value};
use walkdir::WalkDir;

use scanner::{PythonScanner, RustScanner, ScanMode, Scanner, TypeScriptScanner};

/// Sections for `--skip` in Python text mode.
const PY_TEXT_SKIP: &[&str] = &[
    "inventory",
    "complexity",
    "nesting",
    "imports",
    "coupling",
    "cycles",
    "dead-exports",
    "silent-except",
    "todo-audit",
    "decorators",
    "routes",
];

#[derive(Parser, Debug)]
#[command(
    name = "ast-scan",
    version,
    about = "AST-based codebase health scanner (Python, TypeScript, Rust)"
)]
struct Cli {
    /// Root directory to scan
    path: PathBuf,

    #[arg(long, help = "Force Python scanner")]
    python: bool,

    #[arg(long, help = "Force TypeScript / JavaScript scanner")]
    typescript: bool,

    #[arg(long, help = "Force Rust scanner (.rs files)")]
    rust: bool,

    #[arg(
        long,
        help = "Top-level package name for internal imports (Python; default: last segment of resolved path)"
    )]
    pkg: Option<String>,

    #[arg(long, help = "Report title (default: derived from path)")]
    title: Option<String>,

    #[arg(
        long,
        default_value_t = 20,
        help = "How many rows to show in ranked text sections"
    )]
    top: usize,

    #[arg(long, help = "Emit full JSON (--skip applies only to text output)")]
    json: bool,

    #[arg(long, action = clap::ArgAction::Append, help = "Omit SECTION from text report (repeatable)")]
    skip: Vec<String>,

    #[arg(
        long,
        action = clap::ArgAction::Append,
        help = "Exclude paths matching PATTERN (prefix / substring match on path relative to scan root)"
    )]
    exclude: Vec<String>,

    #[arg(long, help = "Exit 1 if any function has cyclomatic complexity > N")]
    max_complexity: Option<u64>,

    #[arg(long, help = "Exit 1 if any function has nesting depth > N")]
    max_nesting: Option<u64>,

    #[arg(long, help = "Exit 1 if circular import count exceeds N")]
    max_cycles: Option<u64>,

    #[arg(
        long,
        default_value = "@/",
        help = "Path alias prefix for TS imports (TypeScript mode; default @/)"
    )]
    alias: String,

    #[arg(
        long,
        help = "Comma-separated ORM method names for camelCase-in-string check (TypeScript; e.g. where,andWhere,select)"
    )]
    orm_check: Option<String>,

    #[arg(
        long,
        action = clap::ArgAction::Append,
        help = "Import boundary: source_prefix:forbidden1,forbidden2 (TypeScript)"
    )]
    boundary: Vec<String>,
}

fn py_skip_valid() -> HashSet<&'static str> {
    PY_TEXT_SKIP.iter().copied().collect()
}

fn should_skip_dir(name: &str) -> bool {
    matches!(
        name,
        "target" | "node_modules" | ".git" | "dist" | "build" | ".venv" | "venv"
    )
}

fn classify_extension(name: &str) -> Option<ScanMode> {
    if name.ends_with(".py") {
        Some(ScanMode::Python)
    } else if name.ends_with(".tsx")
        || name.ends_with(".jsx")
        || (name.ends_with(".ts") && !name.ends_with(".d.ts"))
        || (name.ends_with(".js") && !name.ends_with(".min.js"))
    {
        Some(ScanMode::TypeScript)
    } else if name.ends_with(".rs") {
        Some(ScanMode::Rust)
    } else {
        None
    }
}

/// Walk the tree and return which source kinds exist (Python, TypeScript, Rust order).
fn detect_modes(root: &Path) -> anyhow::Result<Vec<ScanMode>> {
    let mut has_py = false;
    let mut has_ts = false;
    let mut has_rs = false;

    let walker = WalkDir::new(root).into_iter().filter_entry(|e| {
        if e.file_type().is_dir() {
            let n = e.file_name().to_string_lossy();
            return !should_skip_dir(n.as_ref());
        }
        true
    });

    for entry in walker {
        let entry = entry.with_context(|| format!("walk {}", root.display()))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let Some(name) = entry.path().file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        match classify_extension(name) {
            Some(ScanMode::Python) => has_py = true,
            Some(ScanMode::TypeScript) => has_ts = true,
            Some(ScanMode::Rust) => has_rs = true,
            None => {}
        }
        if has_py && has_ts && has_rs {
            break;
        }
    }

    let mut modes = Vec::new();
    if has_py {
        modes.push(ScanMode::Python);
    }
    if has_ts {
        modes.push(ScanMode::TypeScript);
    }
    if has_rs {
        modes.push(ScanMode::Rust);
    }
    if modes.is_empty() {
        anyhow::bail!(
            "No .py, .ts/.tsx/.js/.jsx, or .rs files found under {}",
            root.display()
        );
    }
    Ok(modes)
}

fn explicit_modes(cli: &Cli) -> Option<Vec<ScanMode>> {
    if !cli.python && !cli.typescript && !cli.rust {
        return None;
    }
    let mut modes = Vec::new();
    if cli.python {
        modes.push(ScanMode::Python);
    }
    if cli.typescript {
        modes.push(ScanMode::TypeScript);
    }
    if cli.rust {
        modes.push(ScanMode::Rust);
    }
    Some(modes)
}

fn resolve_modes(cli: &Cli, root: &Path) -> anyhow::Result<Vec<ScanMode>> {
    if let Some(m) = explicit_modes(cli) {
        Ok(m)
    } else {
        detect_modes(root)
    }
}

fn validate_skip(modes: &[ScanMode], skip: &HashSet<String>) -> anyhow::Result<()> {
    let mut valid: HashSet<&str> = HashSet::new();
    for m in modes {
        match m {
            ScanMode::Python => valid.extend(py_skip_valid()),
            ScanMode::TypeScript => valid.extend(ts_scanner::ts_text_skip_sections()),
            ScanMode::Rust => valid.extend(rust_scanner::rs_text_skip_sections()),
        }
    }
    let mut unknown: Vec<&str> = skip
        .iter()
        .filter(|s| !valid.contains(s.as_str()))
        .map(|s| s.as_str())
        .collect();
    if unknown.is_empty() {
        return Ok(());
    }
    unknown.sort();
    let mut valid_list: Vec<_> = valid.iter().copied().collect();
    valid_list.sort();
    anyhow::bail!(
        "ast-scan: unknown --skip section(s): {}. Valid: {}",
        unknown.join(", "),
        valid_list.join(", ")
    )
}

fn default_pkg_py(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("package")
        .to_string()
}

fn default_title_py(pkg: &str) -> String {
    format!("{} — AST ANALYSIS (Python)", pkg.to_uppercase())
}

fn default_title_rs(path: &Path) -> String {
    format!(
        "{} — AST ANALYSIS (Rust)",
        default_project_label_upper(path)
    )
}

fn default_title_ts(path: &Path) -> String {
    format!(
        "{} — AST ANALYSIS (TypeScript)",
        default_project_label_upper(path)
    )
}

/// Uppercase project label from path (parent when basename is `src` / `lib`).
fn default_project_label_upper(path: &Path) -> String {
    let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let base = resolved
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("src");
    let default_name = if base == "src" || base == "lib" {
        resolved
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or(base)
    } else {
        base
    };
    default_name.to_uppercase()
}

fn normalize_alias_prefix(alias: &str) -> String {
    let mut s = alias.trim().to_string();
    if !s.ends_with('/') {
        s.push('/');
    }
    s
}

fn scan_payload_object(data: Value) -> anyhow::Result<Map<String, Value>> {
    let mut obj = match data {
        Value::Object(m) => m,
        _ => anyhow::bail!("scan JSON root must be object"),
    };
    obj.remove("title");
    Ok(obj)
}

fn emit_json(data: Value, report_title: String) -> anyhow::Result<()> {
    let mut obj = scan_payload_object(data)?;
    obj.insert("report_title".into(), Value::String(report_title));
    println!("{}", serde_json::to_string_pretty(&Value::Object(obj))?);
    Ok(())
}

fn emit_multi_json(parts: &[(ScanMode, Value)], report_title: String) -> anyhow::Result<()> {
    let mut root = Map::new();
    root.insert("report_title".into(), Value::String(report_title));
    for &(mode, ref data) in parts {
        let inner = scan_payload_object(data.clone())?;
        root.insert(mode.json_key().to_string(), Value::Object(inner));
    }
    println!("{}", serde_json::to_string_pretty(&Value::Object(root))?);
    Ok(())
}

/// Same JSON keys as Python and TypeScript scanners: `complexity`, `nesting`, `cycles_raw`.
fn check_thresholds(
    data: &Value,
    max_cc: Option<u64>,
    max_nest: Option<u64>,
    max_cyc: Option<u64>,
) -> Vec<String> {
    let mut violations = Vec::new();

    if let Some(limit) = max_cc {
        if let Some(arr) = data["complexity"].as_array() {
            for row in arr {
                let cc = row["cc"].as_u64().unwrap_or(0);
                if cc > limit {
                    violations.push(format!(
                        "CC={cc} exceeds --max-complexity {limit}: {} [{}:{}]",
                        row["name"].as_str().unwrap_or(""),
                        row["file"].as_str().unwrap_or(""),
                        row["line"].as_u64().unwrap_or(0)
                    ));
                    break;
                }
            }
        }
    }

    if let Some(limit) = max_nest {
        if let Some(arr) = data["nesting"].as_array() {
            for row in arr {
                let depth = row["depth"].as_u64().unwrap_or(0);
                if depth > limit {
                    violations.push(format!(
                        "depth={depth} exceeds --max-nesting {limit}: {} [{}:{}]",
                        row["name"].as_str().unwrap_or(""),
                        row["file"].as_str().unwrap_or(""),
                        row["line"].as_u64().unwrap_or(0)
                    ));
                    break;
                }
            }
        }
    }

    if let Some(limit) = max_cyc {
        let cycle_count = data["cycles_raw"].as_array().map(|a| a.len()).unwrap_or(0) as u64;
        if cycle_count > limit {
            violations.push(format!("{cycle_count} cycles exceed --max-cycles {limit}"));
        }
    }

    violations
}

fn build_scanner_runs(
    cli: &Cli,
    modes: &[ScanMode],
    label_base: &str,
) -> Vec<(Box<dyn Scanner>, String)> {
    let multi = modes.len() > 1;

    let ts_in_run = modes.contains(&ScanMode::TypeScript);
    let mut boundary_rules = Vec::new();
    if ts_in_run {
        for raw in &cli.boundary {
            match ts_checks::parse_boundary_flag(raw) {
                Some(r) => boundary_rules.push(r),
                None => {
                    eprintln!(
                        r#"ast-scan: invalid --boundary format: "{raw}". Expected: source_prefix:forbidden1,forbidden2"#
                    );
                    std::process::exit(2);
                }
            }
        }
    }

    let orm_check_methods = cli.orm_check.as_ref().map(|s| {
        s.split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect::<Vec<_>>()
    });

    let ts_cfg = ts_scanner::AnalysisConfig {
        orm_check_methods,
        boundary_rules,
        exclude: cli.exclude.clone(),
    };
    let alias = normalize_alias_prefix(&cli.alias);

    let mut runs: Vec<(Box<dyn Scanner>, String)> = Vec::new();
    for &mode in modes {
        match mode {
            ScanMode::Python => {
                let pkg = cli.pkg.clone().unwrap_or_else(|| default_pkg_py(&cli.path));
                let title = if multi {
                    format!("{} — AST ANALYSIS (Python)", label_base)
                } else {
                    cli.title.clone().unwrap_or_else(|| default_title_py(&pkg))
                };
                runs.push((Box::new(PythonScanner { pkg }), title));
            }
            ScanMode::TypeScript => {
                let title = if multi {
                    format!("{} — AST ANALYSIS (TypeScript)", label_base)
                } else {
                    cli.title
                        .clone()
                        .unwrap_or_else(|| default_title_ts(&cli.path))
                };
                runs.push((
                    Box::new(TypeScriptScanner {
                        alias: alias.clone(),
                        cfg: ts_cfg.clone(),
                    }),
                    title,
                ));
            }
            ScanMode::Rust => {
                let title = if multi {
                    format!("{} — AST ANALYSIS (Rust)", label_base)
                } else {
                    cli.title
                        .clone()
                        .unwrap_or_else(|| default_title_rs(&cli.path))
                };
                runs.push((Box::new(RustScanner), title));
            }
        }
    }
    runs
}

type ScanResult = (Box<dyn Scanner>, Value, String);

fn execute_scans(
    scanner_runs: Vec<(Box<dyn Scanner>, String)>,
    path: &Path,
    exclude: &[String],
) -> anyhow::Result<Vec<ScanResult>> {
    if scanner_runs.len() <= 1 {
        scanner_runs
            .into_iter()
            .map(|(scanner, title)| {
                let data = scanner.analyze(path, exclude)?;
                Ok((scanner, data, title))
            })
            .collect::<anyhow::Result<Vec<_>>>()
    } else {
        std::thread::scope(|s| {
            let handles: Vec<_> = scanner_runs
                .into_iter()
                .map(|(scanner, title)| {
                    s.spawn(move || {
                        let data = scanner.analyze(path, exclude)?;
                        Ok::<_, anyhow::Error>((scanner, data, title))
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("scanner thread panicked"))
                .collect::<anyhow::Result<Vec<_>>>()
        })
    }
}

fn emit_output(
    cli: &Cli,
    results: Vec<ScanResult>,
    label_base: &str,
    skip: &HashSet<String>,
) -> anyhow::Result<()> {
    let multi = results.len() > 1;

    if cli.json {
        let overall_title = if multi {
            format!("{} — AST ANALYSIS (multi-language)", label_base)
        } else {
            results
                .first()
                .map(|(_, _, t)| t.clone())
                .ok_or_else(|| anyhow!("no scan results"))?
        };
        if multi {
            let parts: Vec<(ScanMode, Value)> = results
                .iter()
                .map(|(s, d, _)| (s.mode(), d.clone()))
                .collect();
            emit_multi_json(&parts, overall_title)?;
        } else {
            let (_, data, _) = results
                .into_iter()
                .next()
                .ok_or_else(|| anyhow!("no scan results"))?;
            emit_json(data, overall_title)?;
        }
        return Ok(());
    }

    let mut ts_boundary_fail = false;
    let mut all_violations = Vec::new();

    for (scanner, data, title) in &results {
        let mode = scanner.mode();
        if scanner.print_report(data, title, cli.top, skip) {
            ts_boundary_fail = true;
        }
        all_violations.extend(
            check_thresholds(data, cli.max_complexity, cli.max_nesting, cli.max_cycles)
                .into_iter()
                .map(|v| format!("[{}] {}", mode.threshold_label(), v)),
        );
    }

    if ts_boundary_fail || !all_violations.is_empty() {
        if !all_violations.is_empty() {
            eprintln!();
            for v in all_violations {
                eprintln!("ast-scan: THRESHOLD BREACH: {v}");
            }
        }
        std::process::exit(1);
    }

    Ok(())
}

fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let modes = resolve_modes(&cli, &cli.path)?;

    let skip: HashSet<String> = cli.skip.iter().cloned().collect();
    if let Err(e) = validate_skip(&modes, &skip) {
        eprintln!("{e}");
        std::process::exit(2);
    }

    let label_base = cli
        .title
        .clone()
        .unwrap_or_else(|| default_project_label_upper(&cli.path));

    let scanner_runs = build_scanner_runs(&cli, &modes, &label_base);
    let results = execute_scans(scanner_runs, &cli.path, &cli.exclude)?;
    emit_output(&cli, results, &label_base, &skip)
}

fn main() {
    if let Err(e) = run() {
        eprintln!("ast-scan: {e:#}");
        std::process::exit(1);
    }
}
