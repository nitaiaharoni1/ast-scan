# Rust Rewrite: Parallelism & Threading Guide

Design notes for multithreaded file analysis in the Rust `ast-scan` binary.
**Implemented:** step 2 uses [rayon](https://docs.rs/rayon/) (`par_iter`) in
[`rust/src/ts_scanner/mod.rs`](../rust/src/ts_scanner/mod.rs),
[`rust/src/python_scanner.rs`](../rust/src/python_scanner.rs), and
[`rust/src/rust_scanner/mod.rs`](../rust/src/rust_scanner/mod.rs); aggregation
(steps 3–4) stays sequential. Thread count: `RAYON_NUM_THREADS` or
`ThreadPoolBuilder::build_global`.

## Current Architecture (Rust)

Python, TypeScript, and Rust modes share the same pipeline shape:

```
1. Collect files        (walk directory tree)                    ← sequential
2. Per-file analysis    (read → parse AST → extract metrics)    ← rayon parallel
3. Aggregation          (build import graph, detect cycles, find dead exports)
4. Output               (text report or JSON)
```

Step 2 is the bottleneck. Each file is parsed and analyzed independently — no
file's analysis depends on another file's results. The cross-file work (import
graph, cycles, dead exports, coupling) happens in step 3 *after* all files are
processed.

## Parallelism Strategy

### What can run in parallel

**Per-file analysis (step 2)** is embarrassingly parallel. For each file:

- Read file contents from disk
- Parse source into AST (via `tree-sitter`, `oxc_parser`, or `swc_ecma_parser`)
- Walk AST to extract: functions, classes, imports, exports, complexity,
  nesting, silent catches, decorator usage, hooks, props, etc.
- Collect regex-based findings: TODO comments, eslint-disable comments,
  ts-directives

Each file produces a `FileData` struct. No shared mutable state is needed
during this phase.

### What must stay sequential

**Aggregation (step 3)** consumes all `FileData` results and builds cross-file
structures:

- Import graph (`HashMap<Module, HashSet<Module>>`)
- In-degree / afferent / efferent coupling
- Cycle detection (DFS on the import graph)
- Dead export detection (compare each file's exports against all imported names)
- Hook frequency, external package frequency, etc.

This phase is fast (just iterating collected data, no parsing) and doesn't
benefit much from parallelism. Keep it on the main thread.

## Recommended Implementation

### Crate: `rayon`

Use [rayon](https://docs.rs/rayon/) for data-parallel iteration. It provides a
thread pool and work-stealing scheduler out of the box.

```toml
# Cargo.toml
[dependencies]
rayon = "1"
```

### Core pattern

```rust
use rayon::prelude::*;
use std::path::PathBuf;

/// Per-file analysis result — everything extracted from one source file.
/// This struct must be Send (movable across threads).
struct FileData {
    rel_path: String,
    line_count: usize,
    functions: Vec<FuncInfo>,
    classes: Vec<ClassInfo>,
    imports: Vec<ImportInfo>,
    exports: Vec<String>,
    complexity_entries: Vec<ComplexityEntry>,
    silent_catches: Vec<SilentCatchInfo>,
    // ... other per-file findings
}

/// Analyze a single file. Pure function — no shared state.
fn analyze_file(path: &PathBuf, scan_root: &Path, config: &Config) -> Result<FileData> {
    let source = std::fs::read_to_string(path)?;
    let ast = parse_source(&source, path)?;    // oxc_parser or tree-sitter
    
    let functions = extract_functions(&ast);
    let imports = extract_imports(&ast, scan_root, &config.alias_prefix);
    let exports = extract_exports(&ast);
    let silent_catches = find_silent_catches(&ast);
    // ... etc
    
    Ok(FileData { /* ... */ })
}

fn main() {
    let files: Vec<PathBuf> = collect_files(&scan_root, &exclude_patterns);
    
    // ── Step 2: parallel per-file analysis ──
    let results: Vec<FileData> = files
        .par_iter()                     // rayon parallel iterator
        .filter_map(|f| {
            match analyze_file(f, &scan_root, &config) {
                Ok(data) => Some(data),
                Err(e) => {
                    eprintln!("  SKIP: {}: {}", f.display(), e);
                    None
                }
            }
        })
        .collect();
    
    // ── Step 3: sequential aggregation ──
    let report = aggregate(results, &config);
    
    // ── Step 4: output ──
    if config.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_text_report(&report, &config);
    }
}
```

### Key design rules

1. **`analyze_file` must be a pure function.** It takes a file path and
   read-only config, returns a `FileData`. No globals, no shared mutables, no
   `Arc<Mutex<...>>` needed.

2. **All types in `FileData` must be `Send`.** This is automatic for owned
   types (`String`, `Vec`, etc.). Avoid `Rc`, raw pointers, or references into
   shared state.

3. **File I/O happens inside each task.** Each thread reads its own file.
   Don't pre-read all files into memory — the OS page cache handles this
   efficiently, and it avoids a memory spike on large codebases.

4. **Error handling per file.** If a file fails to parse (syntax error, encoding
   issue), log and skip it. Don't let one bad file kill the whole scan. Use
   `filter_map` as shown above.

## Parser Choice

| Crate | Language | Speed | Notes |
|-------|----------|-------|-------|
| `oxc_parser` | TS/JS/TSX/JSX | Fastest (3-5x faster than SWC) | Returns oxc AST, good visitor API |
| `swc_ecma_parser` | TS/JS/TSX/JSX | Very fast | Mature, widely used |
| `tree-sitter` + `tree-sitter-typescript` | TS/JS | Fast | Language-agnostic, incremental |
| `rustpython-parser` | Python | Fast | Produces Python AST in Rust |
| `tree-sitter` + `tree-sitter-python` | Python | Fast | Alternative for Python |
| `ruff_python_parser` | Python | Fastest for Python | Used by ruff, well-maintained |

**Recommendation:** Use `oxc_parser` for TypeScript/JSX and
`ruff_python_parser` for Python. Both are actively maintained, fast, and
produce ASTs that are easy to walk.

## Thread Count

Rayon auto-detects CPU count. For ast-scan this is fine — the work is
CPU-bound (parsing + tree walking) with small I/O per task.

To override (e.g., for CI where you want to limit cores):

```rust
rayon::ThreadPoolBuilder::new()
    .num_threads(num_cpus)
    .build_global()
    .unwrap();
```

Or let users set `RAYON_NUM_THREADS=4` as an environment variable (rayon
respects this automatically).

## What NOT to parallelize

- **Directory walking** (`collect_files`): Fast enough sequentially. Parallelizing
  adds complexity for negligible gain. Use `walkdir` or `ignore` crate.
- **Cycle detection**: DFS on the import graph. The graph is small (nodes =
  number of files, not lines). Takes microseconds.
- **Dead export resolution**: Single pass over collected data. Trivially fast.
- **Text report printing**: Must be sequential (stdout is ordered).

## Memory Considerations

Each thread holds one file's source string + AST in memory at a time. Rayon's
work-stealing means at most `num_threads` files are in-flight simultaneously.
For a typical 8-core machine scanning 10,000 files averaging 200 lines each,
peak memory for the parallel phase is roughly:

```
8 threads × (~50KB source + ~200KB AST) ≈ 2MB
```

The `Vec<FileData>` results accumulate, but `FileData` doesn't store the
source text or AST — only extracted metrics (small structs and strings).

## Aggregation Phase Detail

After `par_iter().collect()` returns all `FileData`, build the cross-file
structures:

```rust
fn aggregate(results: Vec<FileData>, config: &Config) -> Report {
    let mut graph: HashMap<String, HashSet<String>> = HashMap::new();
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    let mut imported_names: HashMap<String, HashSet<String>> = HashMap::new();
    let mut all_functions: Vec<FuncInfo> = Vec::new();
    let mut all_exports: Vec<(String, Vec<String>)> = Vec::new();
    // ... etc

    for fd in &results {
        all_functions.extend(fd.functions.iter().cloned());
        
        let from_mod = normalize_module_path(&fd.rel_path);
        all_exports.push((from_mod.clone(), fd.exports.clone()));
        
        for imp in &fd.imports {
            if !imp.is_internal { continue; }
            let to_mod = normalize_module_path(&imp.resolved_path);
            graph.entry(from_mod.clone()).or_default().insert(to_mod.clone());
            *in_degree.entry(to_mod.clone()).or_default() += 1;
            
            let names = imported_names.entry(to_mod).or_default();
            for spec in &imp.specifiers {
                names.insert(spec.clone());
            }
        }
    }

    let cycles = find_cycles(&graph);
    let dead_exports = find_dead_exports(&all_exports, &imported_names);
    let coupling = compute_coupling(&graph);
    
    // Sort and build final report
    Report { /* ... */ }
}
```

## Testing the Parallelism

1. **Correctness**: Run on the same codebase with `RAYON_NUM_THREADS=1` (sequential)
   and default (parallel). JSON output must be identical (sort arrays first
   since parallel order is nondeterministic).

2. **Benchmarking**: Use `hyperfine` to compare:
   ```bash
   hyperfine \
     'RAYON_NUM_THREADS=1 ast-scan ./big-project --json > /dev/null' \
     'ast-scan ./big-project --json > /dev/null'
   ```

3. **Large-scale test**: Find or generate a 10,000+ file TypeScript project.
   The parallel version should show near-linear speedup up to ~8 cores, then
   diminishing returns as I/O becomes the bottleneck.

## Summary

```
┌──────────────────┐
│  collect_files()  │  sequential, walkdir crate
└────────┬─────────┘
         │  Vec<PathBuf>
         ▼
┌──────────────────┐
│  files.par_iter() │  PARALLEL — rayon
│  .map(analyze)    │  each thread: read → parse → extract metrics
│  .collect()       │  returns Vec<FileData>
└────────┬─────────┘
         │  Vec<FileData>
         ▼
┌──────────────────┐
│   aggregate()     │  sequential — build graph, detect cycles,
│                   │  find dead exports, compute coupling
└────────┬─────────┘
         │  Report
         ▼
┌──────────────────┐
│   output()        │  sequential — print text or JSON
└──────────────────┘
```

The core change is `.iter()` → `.par_iter()` once per-file work is a pure
function returning owned `Send` data (Python needed a `PyFileData` bundle and
`Box` on the scan enum for ergonomics and small stack frames). New scanners
should follow the same collect → parallel analyze → sequential aggregate split.
