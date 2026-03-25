# Feature Batch Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add five improvements to ast-scan: extended secret patterns, Type-1 (exact) clone detection, function line-count in complexity rows + `--max-lines` threshold, Python mutable default argument detection, and dead-export false-positive reduction via star-import/re-export tracking (Python + TypeScript).

**Architecture:** Each feature is additive — new fields on existing structs, new JSON keys, new CLI flags. No existing behaviour is removed. The pipeline is: per-file analysis (parallel) → aggregation (sequential) → JSON output. Features slot in at the analysis or aggregation step depending on where data is available.

**Tech Stack:** Rust 2021, clap 4, rayon, rustpython-parser 0.4, oxc_parser 0.122, serde_json.

---

## File Map

| File | Change |
|------|--------|
| `rust/src/secrets.rs` | Add 6 new regex patterns |
| `rust/src/clones.rs` | Add `hash_exact` (whitespace-normalised raw text hash) |
| `rust/src/types.rs` | Add `exact_clone_hash: u64` to `PyFuncInfo`/`TsFuncInfo`; add `MutableDefaultInfo`; add `star_imported_modules: Vec<String>` to `PyFileData`; add `star_reexport_sources: Vec<String>` + `namespace_import_sources: Vec<String>` to `TsFileData` |
| `rust/src/python_scanner/visitors.rs` | Add `python_func_exact_hash`, `collect_mutable_defaults` |
| `rust/src/python_scanner/file.rs` | Wire `exact_clone_hash`, `mutable_defaults`, `star_imported_modules` into `FileAnalyzer` |
| `rust/src/python_scanner/mod.rs` | Build `type1_clones` JSON; add `mutable_defaults` JSON; fix dead-export logic to skip star-consumed modules; add `lines` to complexity rows |
| `rust/src/ts_scanner/file.rs` | Wire `exact_clone_hash` from OXC span; detect `export * from` / `import * as`; add fields to `TsFileData` |
| `rust/src/ts_scanner/mod.rs` | Build `type1_clones` JSON; add `lines` to complexity rows; fix dead-export logic for star-consumed modules |
| `rust/src/main.rs` | Add `--max-lines` to `Cli`; add branch in `check_thresholds`; add `"mutable-defaults"` to `PY_TEXT_SKIP` |
| `rust/src/report/python.rs` | Add `mutable_defaults` text section |
| `rust/tests/integration.rs` | Tests for each new feature |
| `fixtures/minimal-py/star_consumer.py` | New fixture: `from pkg.util import *` |
| `fixtures/minimal-py/mutable_defaults.py` | New fixture: function with list/dict/set defaults |

---

## Task 1: Extend secret patterns

**Files:**
- Modify: `rust/src/secrets.rs`

Add six new `OnceLock<Regex>` helpers and integrate them into `audit_string_literal`.

- [ ] **Add new regex helpers after `re_github`:**

```rust
fn re_jwt() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"^eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}$")
            .expect("jwt regex")
    })
}

fn re_slack() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"^xox[baprs]-[0-9A-Za-z-]{10,}$").expect("slack regex")
    })
}

fn re_google_api() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"^AIza[0-9A-Za-z\-_]{35}$").expect("google api regex")
    })
}

fn re_sendgrid() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"^SG\.[A-Za-z0-9_-]{22}\.[A-Za-z0-9_-]{43}$").expect("sendgrid regex")
    })
}

fn re_pem_header() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"-----BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY-----")
            .expect("pem regex")
    })
}

fn re_db_url() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(postgres|mysql|mongodb)://[^:@\s]{1,64}:[^@\s]{1,128}@")
            .expect("db url regex")
    })
}
```

- [ ] **Add match blocks in `audit_string_literal` after the GitHub check:**

```rust
if re_jwt().is_match(trimmed) {
    return Some(SecurityFinding {
        kind: "jwt_token".into(),
        file: file.into(),
        line,
        detail: "Possible hardcoded JWT token".into(),
    });
}
if re_slack().is_match(trimmed) {
    return Some(SecurityFinding {
        kind: "slack_token".into(),
        file: file.into(),
        line,
        detail: "Possible Slack API token (xox...)".into(),
    });
}
if re_google_api().is_match(trimmed) {
    return Some(SecurityFinding {
        kind: "google_api_key".into(),
        file: file.into(),
        line,
        detail: "Possible Google API key (AIza...)".into(),
    });
}
if re_sendgrid().is_match(trimmed) {
    return Some(SecurityFinding {
        kind: "sendgrid_api_key".into(),
        file: file.into(),
        line,
        detail: "Possible SendGrid API key (SG....)".into(),
    });
}
if re_pem_header().is_match(trimmed) {
    return Some(SecurityFinding {
        kind: "private_key_pem".into(),
        file: file.into(),
        line,
        detail: "Possible PEM private key header in string literal".into(),
    });
}
if re_db_url().is_match(trimmed) {
    return Some(SecurityFinding {
        kind: "db_connection_string".into(),
        file: file.into(),
        line,
        detail: "Possible database URL with embedded credentials".into(),
    });
}
```

- [ ] **Build and verify no compile errors:**
```bash
cd rust && cargo build 2>&1 | head -30
```

- [ ] **Commit:**
```bash
git add rust/src/secrets.rs
git commit -m "feat: add JWT, Slack, Google, SendGrid, PEM, DB-URL secret patterns"
```

---

## Task 2: Type-1 (exact) clone detection

Type-2 clones (existing) use a normalised AST shape hash — identifiers/literals stripped. Type-1 adds an exact-text hash (whitespace normalised, identifiers preserved), detecting copy-paste duplicates that might differ only in whitespace.

**Files:**
- Modify: `rust/src/clones.rs`
- Modify: `rust/src/types.rs`
- Modify: `rust/src/python_scanner/visitors.rs`
- Modify: `rust/src/python_scanner/file.rs`
- Modify: `rust/src/python_scanner/mod.rs`
- Modify: `rust/src/ts_scanner/file.rs`
- Modify: `rust/src/ts_scanner/mod.rs`

### 2a — `clones.rs`: add `hash_exact`

- [ ] **Add after `hash_shape`:**

```rust
/// Hash a raw source slice normalised for whitespace only (Type-1 clone detection).
/// Each line is trimmed; blank lines removed; joined with `\n`.
pub(crate) fn hash_exact(payload: &str) -> u64 {
    let normalized: String = payload
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let mut h = DefaultHasher::new();
    normalized.hash(&mut h);
    h.finish()
}
```

### 2b — `types.rs`: add `exact_clone_hash` field

- [ ] **In `PyFuncInfo`, add after `clone_hash`:**
```rust
/// Exact text hash (whitespace-normalised) for Type-1 clone detection.
pub exact_clone_hash: u64,
```

- [ ] **In `TsFuncInfo`, add after `clone_hash`:**
```rust
pub exact_clone_hash: u64,
```

### 2c — `python_scanner/visitors.rs`: add `python_func_exact_hash`

- [ ] **Add at the bottom of the file:**

```rust
/// Hash the raw source slice of a function (whitespace-normalised) for Type-1 clone detection.
pub(super) fn python_func_exact_hash(source: &str, range_start: usize, range_end: usize) -> u64 {
    let slice = source.get(range_start..range_end.min(source.len())).unwrap_or("");
    crate::clones::hash_exact(slice)
}
```

### 2d — `python_scanner/file.rs`: wire in exact hash

- [ ] **Add import in the `use super::visitors` block:**
```rust
python_func_exact_hash,
```

- [ ] **In `process_function`, after `let clone_hash = python_body_shape_hash(...)`:**, add:
```rust
let start = usize::from(node.range().start());
let end = usize::from(node.range().end());
let exact_clone_hash = python_func_exact_hash(self.source, start, end);
```

- [ ] **Add `exact_clone_hash` to the `PyFuncInfo { ... }` struct literal.**

- [ ] **Repeat the same two steps in `process_async_function`.**

### 2e — `python_scanner/mod.rs`: build `type1_clones`

- [ ] **Add `build_type1_clones_py` after `build_code_clones_py`:**

```rust
fn build_type1_clones_py(
    scan_root: &Path,
    funcs: &[crate::types::PyFuncInfo],
) -> Vec<serde_json::Value> {
    let mut m: HashMap<u64, Vec<&crate::types::PyFuncInfo>> = HashMap::new();
    for f in funcs {
        if f.line_count > CLONE_MIN_LINES {  // `>` matches existing build_code_clones_py
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
```

- [ ] **In `build_json`, after `let code_clones = build_code_clones_py(...)`, add:**
```rust
let type1_clones = build_type1_clones_py(scan_root, &cd.all_functions);
```

- [ ] **Add `"type1_clones": type1_clones,` to the `serde_json::json!({...})` output block.**

### 2f — `ts_scanner/file.rs`: wire exact hash for TS functions

The `analyze_ts_file` function has access to `source` and builds `TsFuncInfo` entries. Spans are available on OXC AST nodes.

- [ ] **Add a file-local helper near the top of `ts_scanner/file.rs`:**

```rust
fn exact_hash_from_span(source: &str, start: u32, end: u32) -> u64 {
    let s = start as usize;
    let e = (end as usize).min(source.len());
    crate::clones::hash_exact(source.get(s..e).unwrap_or(""))
}
```

- [ ] **Find every place a `TsFuncInfo { ... }` struct literal is constructed in `analyze_ts_file`. For each, compute `exact_clone_hash` from the node's span and add the field.** The pattern will be:
```rust
let exact_clone_hash = exact_hash_from_span(&source, node.span.start, node.span.end);
// ... TsFuncInfo { ..., exact_clone_hash }
```

> Note: OXC `Function` nodes expose `.span`; `ArrowFunctionExpression` also has `.span`.

### 2g — `ts_scanner/mod.rs`: build TS `type1_clones`

- [ ] **Add `build_type1_clones_ts` after `build_code_clones_ts`:**

```rust
fn build_type1_clones_ts(funcs: &[crate::types::TsFuncInfo]) -> Vec<Value> {
    let mut m: HashMap<u64, Vec<&crate::types::TsFuncInfo>> = HashMap::new();
    for f in funcs {
        if f.line_count > CLONE_MIN_LINES_TS {  // `>` matches existing build_code_clones_ts
            m.entry(f.exact_clone_hash).or_default().push(f);
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
```

- [ ] **In `build_json`, after `let code_clones = build_code_clones_ts(...)`, add:**
```rust
let type1_clones = build_type1_clones_ts(&all_functions);
```

- [ ] **Add `"type1_clones": type1_clones,` to the `Ok(json!({...}))` block.**

- [ ] **Build:**
```bash
cd rust && cargo build 2>&1 | head -40
```

- [ ] **Commit:**
```bash
git add rust/src/clones.rs rust/src/types.rs \
        rust/src/python_scanner/visitors.rs rust/src/python_scanner/file.rs rust/src/python_scanner/mod.rs \
        rust/src/ts_scanner/file.rs rust/src/ts_scanner/mod.rs
git commit -m "feat: Type-1 (exact text) clone detection for Python and TypeScript"
```

---

## Task 3: Function line count in complexity rows + `--max-lines`

`line_count` is already on `PyFuncInfo` and `TsFuncInfo`; it just isn't included in the `complexity` JSON array rows. Add it so consumers can rank by lines, and add a `--max-lines` CI threshold.

**Files:**
- Modify: `rust/src/python_scanner/mod.rs`
- Modify: `rust/src/ts_scanner/mod.rs`
- Modify: `rust/src/main.rs`

### 3a — Python complexity rows

- [ ] **In `python_scanner/mod.rs`, `build_json`, find the `complexity_rows` mapping. Add `"lines": fn_.line_count,` to each row:**

```rust
// Before (excerpt):
"cc": fn_.complexity,

// After:
"cc": fn_.complexity,
"lines": fn_.line_count,
```

### 3b — TypeScript complexity rows

- [ ] **In `ts_scanner/mod.rs`, `build_json`, find the `complexity_rows` mapping. Add `"lines": fn_.line_count,`:**

```rust
"cc": fn_.complexity,
"lines": fn_.line_count,
```

### 3c — `--max-lines` CLI flag + threshold check

Steps must be done in order — field must exist before updating signature.

- [ ] **Step 1 — In `main.rs` `Cli` struct, add after `max_cycles`:**

```rust
#[arg(long, help = "Exit 1 if any function exceeds N lines")]
max_lines: Option<u64>,
```

- [ ] **Step 2 — In `check_thresholds` signature, add `max_lines: Option<u64>` parameter.**

- [ ] **Step 3 — In `check_thresholds` body, add after the `max_params` block:**

```rust
if let Some(limit) = max_lines {
    if let Some(arr) = data["complexity"].as_array() {
        for row in arr {
            let lines = row["lines"].as_u64().unwrap_or(0);
            if lines > limit {
                violations.push(format!(
                    "lines={lines} exceeds --max-lines {limit}: {} [{}:{}]",
                    row["name"].as_str().unwrap_or(""),
                    row["file"].as_str().unwrap_or(""),
                    row["line"].as_u64().unwrap_or(0)
                ));
                break;
            }
        }
    }
}
```

- [ ] **Step 4 — Find all call sites of `check_thresholds` in `main.rs` (there is exactly one, inside `emit_output`) and add `cli.max_lines` as the new argument.**

- [ ] **Build:**
```bash
cd rust && cargo build 2>&1 | head -40
```

- [ ] **Commit:**
```bash
git add rust/src/python_scanner/mod.rs rust/src/ts_scanner/mod.rs rust/src/main.rs
git commit -m "feat: add lines field to complexity rows and --max-lines threshold"
```

---

## Task 4: Mutable default arguments (Python)

Detect `def foo(x=[], y={}, z=set())` — a classic Python footgun.

**Files:**
- Modify: `rust/src/types.rs`
- Modify: `rust/src/python_scanner/visitors.rs`
- Modify: `rust/src/python_scanner/file.rs`
- Modify: `rust/src/python_scanner/mod.rs`
- Modify: `rust/src/main.rs`
- Modify: `rust/src/report/python.rs`
- Create: `fixtures/minimal-py/mutable_defaults.py`

### 4a — `types.rs`: add `MutableDefaultInfo` and extend `PyFileData`

- [ ] **Add after `RouteInfo`:**

```rust
#[derive(Debug, Clone, Serialize)]
pub(crate) struct MutableDefaultInfo {
    pub file: String,
    pub line: usize,
    pub func_name: String,
    pub param_name: String,
    /// "list", "dict", "set", or "call"
    pub kind: String,
}
```

- [ ] **In `PyFileData`, add:**
```rust
pub mutable_defaults: Vec<MutableDefaultInfo>,
```

### 4b — `python_scanner/visitors.rs`: add `collect_mutable_defaults`

- [ ] **Add at the bottom of the file:**

```rust
fn mutable_default_kind(expr: &Expr) -> Option<&'static str> {
    match expr {
        Expr::List(_) => Some("list"),
        Expr::Dict(_) => Some("dict"),
        Expr::Set(_) => Some("set"),
        Expr::Call(_) => Some("call"),
        _ => None,
    }
}

/// Collect function arguments that have mutable default values.
pub(super) fn collect_mutable_defaults(
    args: &Arguments,
    func_name: &str,
    file: &str,
    source: &str,
) -> Vec<crate::types::MutableDefaultInfo> {
    let mut out = Vec::new();
    // Note: field is `.def` (not `.def_`) in rustpython-parser 0.4
    for arged in args.posonlyargs.iter().chain(args.args.iter()) {
        if let Some(default) = &arged.default {
            if let Some(kind) = mutable_default_kind(default) {
                let line = line_at(source, default.range().start());
                out.push(crate::types::MutableDefaultInfo {
                    file: file.to_string(),
                    line,
                    func_name: func_name.to_string(),
                    param_name: arged.def.arg.to_string(),
                    kind: kind.to_string(),
                });
            }
        }
    }
    for (i, default_opt) in args.kw_defaults.iter().enumerate() {
        if let Some(default) = default_opt {
            if let Some(kind) = mutable_default_kind(default) {
                let param_name = args
                    .kwonlyargs
                    .get(i)
                    .map(|a| a.def.arg.to_string())
                    .unwrap_or_else(|| format!("kwarg_{i}"));
                let line = line_at(source, default.range().start());
                out.push(crate::types::MutableDefaultInfo {
                    file: file.to_string(),
                    line,
                    func_name: func_name.to_string(),
                    param_name,
                    kind: kind.to_string(),
                });
            }
        }
    }
    out
}
```

### 4c — `python_scanner/file.rs`: wire `collect_mutable_defaults`

- [ ] **Add `collect_mutable_defaults` to the `use super::visitors` import.**

- [ ] **In `FileAnalyzer`, add field:**
```rust
mutable_defaults: Vec<crate::types::MutableDefaultInfo>,
```
and initialise it as `mutable_defaults: Vec::new()` in `new`.

- [ ] **In `process_function`, after building `info`, add:**
```rust
let mut mds = collect_mutable_defaults(
    &node.args,
    &qualname,
    self.filepath,
    self.source,
);
self.mutable_defaults.append(&mut mds);
```

- [ ] **Repeat in `process_async_function`.**

- [ ] **In `analyze_py_file`, thread `an.mutable_defaults` into `PyFileData`:**
```rust
mutable_defaults: an.mutable_defaults,
```

### 4d — `python_scanner/mod.rs`: aggregate and emit JSON

- [ ] **In `PyCollectedData`, add:**
```rust
all_mutable_defaults: Vec<crate::types::MutableDefaultInfo>,
```

- [ ] **In `collect_and_aggregate`, initialise `all_mutable_defaults: Vec::new()` and extend it:**
```rust
all_mutable_defaults.extend(fd.mutable_defaults);
```
Include in the returned struct.

- [ ] **In `build_json`, after building `security_sorted`, add:**
```rust
let mut mutable_defaults = cd.all_mutable_defaults;
mutable_defaults.sort_by(|a, b| a.file.cmp(&b.file).then_with(|| a.line.cmp(&b.line)));
```

- [ ] **Add to the JSON output:**
```rust
"mutable_defaults": serde_json::to_value(&mutable_defaults).unwrap_or(serde_json::Value::Null),
```

### 4e — `main.rs`: register `mutable-defaults` skip section

- [ ] **Add `"mutable-defaults"` to `PY_TEXT_SKIP`.**

### 4f — `report/python.rs`: add text section

Find where `silent_excepts` is printed (as a reference for similar section style) and add a `mutable_defaults` section nearby:

- [ ] **Add a section that prints `mutable_defaults` findings, skippable via `"mutable-defaults"`, showing `file:line func_name(param_name=<kind>)`.**

### 4g — Fixture

- [ ] **Create `fixtures/minimal-py/mutable_defaults.py`:**

```python
def bad_defaults(items=[], mapping={}, unique=set()):
    items.append(1)
    return items
```

- [ ] **Build:**
```bash
cd rust && cargo build 2>&1 | head -40
```

- [ ] **Commit:**
```bash
git add rust/src/types.rs \
        rust/src/python_scanner/visitors.rs \
        rust/src/python_scanner/file.rs \
        rust/src/python_scanner/mod.rs \
        rust/src/main.rs \
        rust/src/report/python.rs \
        fixtures/minimal-py/mutable_defaults.py
git commit -m "feat: detect mutable default arguments in Python functions"
```

---

## Task 5: Reduce dead-export false positives

**Problem:**
- Python: `from module import *` makes all exports of `module` "used", but the current code marks them as dead.
- TypeScript: `export * from './foo'` re-exports everything from `foo`; `import * as X from './foo'` consumes everything. Both cause false positives.

**Files:**
- Modify: `rust/src/types.rs`
- Modify: `rust/src/python_scanner/visitors.rs`
- Modify: `rust/src/python_scanner/file.rs`
- Modify: `rust/src/python_scanner/mod.rs`
- Modify: `rust/src/ts_scanner/file.rs`
- Modify: `rust/src/ts_scanner/mod.rs`
- Create: `fixtures/minimal-py/star_consumer.py`

### 5a — Python: track star imports

- [ ] **In `types.rs` `PyFileData`, add:**
```rust
/// Modules that this file star-imports (`from X import *`).
pub star_imported_modules: Vec<String>,
```

- [ ] **In `python_scanner/visitors.rs`, add `collect_star_import`:**

```rust
/// If `stmt` is `from X import *`, return the fully-qualified module name.
pub(super) fn collect_star_import(stmt: &Stmt, pkg: &str) -> Option<String> {
    if let Stmt::ImportFrom(imp) = stmt {
        // Check that names is exactly [*]
        if imp.names.len() == 1 && imp.names[0].name.as_str() == "*" {
            let module_str = imp.module.as_ref()?.as_str();
            if imp.level.is_some_and(|l| usize::from(l) > 0) {
                // Relative import — treat as internal
                return Some(format!("{}.{}", pkg, module_str));
            }
            return Some(module_str.to_string());
        }
    }
    None
}
```

- [ ] **In `python_scanner/file.rs`, add `collect_star_import` to visitors imports and add `star_imported_modules: Vec<String>` to `FileAnalyzer`. In `visit_module`, after calling `process_import`, also call:**
```rust
if let Some(star_mod) = collect_star_import(item, self.pkg) {
    self.star_imported_modules.push(star_mod);
}
```

- [ ] **Thread `an.star_imported_modules` into `PyFileData`.**

### 5b — Python: use star imports in dead-export detection

- [ ] **In `python_scanner/mod.rs` `PyCollectedData`, add:**
```rust
star_imported_modules: Vec<String>,
```
Aggregate it in `collect_and_aggregate` (extend from each `fd.star_imported_modules`).

- [ ] **In `build_json`, before the dead-export loop, build a `star_consumed` set:**
```rust
let star_consumed: HashSet<String> = cd.star_imported_modules.into_iter().collect();
```

- [ ] **In the dead-export loop, skip modules that are star-consumed:**
```rust
for (mod_name, names) in &cd.module_top_names {
    if star_consumed.contains(mod_name) {
        continue; // all exports potentially consumed via `import *`
    }
    // ... existing dead export logic unchanged
}
```

### 5c — TypeScript: track `export *` and `import * as`

- [ ] **In `types.rs` `TsFileData`, add:**
```rust
/// Resolved paths of modules this file re-exports with `export * from '...'`.
pub star_reexport_sources: Vec<String>,
/// Resolved paths of modules consumed via `import * as X from '...'`.
pub namespace_import_sources: Vec<String>,
```

- [ ] **In `ts_scanner/file.rs`, in `analyze_ts_file`, declare two new local vecs before the program walk:**
```rust
let mut star_reexport_sources: Vec<String> = Vec::new();
```

- [ ] **In the program walk's `for stmt in &program.body { match stmt { ... } }` block (around line 695), add a new arm for `ExportAllDeclaration`. This is currently swallowed by `_ => {}`:**
```rust
Statement::ExportAllDeclaration(ex) => {
    let imp_path = ex.source.value.as_str();
    let (is_internal, resolved) =
        resolve_import(imp_path, &abs, scan_root, alias_prefix);
    if is_internal {
        star_reexport_sources.push(resolved);
    }
}
```

- [ ] **For `import * as X`, do NOT change `ingest_import_declaration`'s signature. Instead, after the program walk loop, derive `namespace_import_sources` from the already-built `imports` vec. The existing `ImportNamespaceSpecifier` branch (line ~282) already records specifiers as `"* as X"`. Use this:**
```rust
let namespace_import_sources: Vec<String> = imports
    .iter()
    .filter(|i| i.is_internal && i.specifiers.iter().any(|s| s.starts_with("* as ")))
    .map(|i| i.resolved_path.clone())
    .collect();
```

- [ ] **Add both to the `TsFileData { ... }` return literal:**
```rust
star_reexport_sources,
namespace_import_sources,
```

### 5d — TypeScript: use them in `find_dead_exports`

- [ ] **In `ts_scanner/mod.rs`, add `star_consumed` field to the `TsImportGraph` struct:**

```rust
struct TsImportGraph {
    graph: HashMap<String, HashSet<String>>,
    in_degree: HashMap<String, usize>,
    all_modules: HashSet<String>,
    imported_names_map: HashMap<String, HashSet<String>>,
    /// Modules fully consumed via `export *` or `import * as`.
    star_consumed: HashSet<String>,
}
```

- [ ] **In `build_import_graph`, build `star_consumed` before the return and include it in the return literal:**

```rust
let star_consumed: HashSet<String> = all_data
    .iter()
    .flat_map(|d| {
        d.star_reexport_sources.iter().chain(d.namespace_import_sources.iter())
    })
    .map(|s| file::normalize_module_path(s))
    .collect();

TsImportGraph {
    graph,
    in_degree,
    all_modules,
    imported_names_map,
    star_consumed,   // ← new field
}
```

- [ ] **In `find_dead_exports`, add `star_consumed: &HashSet<String>` parameter and skip star-consumed modules:**

```rust
fn find_dead_exports(
    all_data: &[TsFileData],
    root: &Path,
    imported_names_map: &HashMap<String, HashSet<String>>,
    star_consumed: &HashSet<String>,  // ← new
) -> Vec<(String, String)> {
    // ...
    for d in all_data {
        // ...
        if star_consumed.contains(&rel_norm) {
            continue; // all exports potentially consumed via re-export or namespace import
        }
        // ... existing dead export logic unchanged
    }
}
```

- [ ] **Update the one call site of `find_dead_exports` in `analyze_typescript` (in `build_json` or the top-level function — currently: `find_dead_exports(&all_data, &root, &ig.imported_names_map)`). Pass `&ig.star_consumed` as the fourth argument:**

```rust
let dead_exports = find_dead_exports(&all_data, &root, &ig.imported_names_map, &ig.star_consumed);
```

### 5e — Fixture for Python star import

- [ ] **Create `fixtures/minimal-py/star_consumer.py`:**
```python
from pkg.util import *
```

- [ ] **Build:**
```bash
cd rust && cargo build 2>&1 | head -40
```

- [ ] **Commit:**
```bash
git add rust/src/types.rs \
        rust/src/python_scanner/visitors.rs \
        rust/src/python_scanner/file.rs \
        rust/src/python_scanner/mod.rs \
        rust/src/ts_scanner/file.rs \
        rust/src/ts_scanner/mod.rs \
        fixtures/minimal-py/star_consumer.py
git commit -m "fix: reduce dead-export false positives from star imports and re-exports"
```

---

## Task 6: Integration tests + full test pass

**Files:**
- Modify: `rust/tests/integration.rs`

- [ ] **Add tests at the bottom of the file:**

```rust
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
```

- [ ] **Run all tests:**
```bash
cd rust && cargo test 2>&1
```
Expected: all pass. Fix any failures before proceeding.

- [ ] **Run clippy:**
```bash
cd rust && cargo clippy -- -D warnings 2>&1
```
Fix any warnings.

- [ ] **Dogfood — scan the project's own source:**
```bash
./target/debug/ast-scan rust/src --rust --json > /dev/null && echo OK
```

- [ ] **Commit:**
```bash
git add rust/tests/integration.rs fixtures/
git commit -m "test: integration tests for lines field, type1 clones, mutable defaults, max-lines threshold"
```
