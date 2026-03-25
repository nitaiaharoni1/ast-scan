# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What This Is

**ast-scan** is a single Rust binary that performs AST-level analysis across Python, TypeScript/JavaScript, and Rust codebases. It produces health reports covering complexity, coupling, circular imports, dead exports, secrets, and language-specific audits.

## Commands

All commands run from the `rust/` directory (or the workspace root — same effect due to workspace layout).

```bash
# Build
cargo build --release          # binary at target/release/ast-scan

# Test
cargo test                     # all tests (integration + unit)
cargo test json_python         # run a single integration test by name

# Lint
cargo clippy -- -D warnings    # strict (matches CI)

# Run
./target/release/ast-scan rust/src --rust
./target/release/ast-scan . --json
```

CI runs: clippy → test → build → dogfood (`ast-scan rust/src --rust --json`).

## Architecture

### Core Pipeline (same for all three languages)

```
1. Collect files       (sequential, walkdir)
2. Per-file analysis   (rayon parallel)  →  FileData structs
3. Aggregation         (sequential)      →  graphs, cycles, coupling, dead exports
4. JSON serialize      (serde_json)      →  Value (printed or emitted)
```

### Key Modules

| Module | Role |
|--------|------|
| `src/main.rs` | CLI (clap), mode detection, scanner orchestration, threshold checking |
| `src/scanner.rs` | `Scanner` trait — uniform interface across languages |
| `src/types.rs` | Shared + language-specific data structs (all `#[derive(Serialize)]`) |
| `src/graph.rs` | Cycle detection (DFS), coupling metrics (Ca/Ce/Instability) |
| `src/audits.rs` | Regex-based TODO/FIXME/eslint-disable/`@ts-*` collection |
| `src/secrets.rs` | Hardcoded secret heuristics (pattern + entropy) |
| `src/clones.rs` | Type-2 clone detection via normalized AST shape hashing |
| `src/{python,ts,rust}_scanner/` | Language scanner trios: `mod.rs`, `file.rs`, `visitors.rs` |
| `src/report/{python,typescript,rust_report}.rs` | Text report rendering; JSON is always emitted regardless |

### Scanner Modules (`python_scanner/`, `ts_scanner/`, `rust_scanner/`)

Each follows the same internal layout:
- **`mod.rs`** — entry point, parallel dispatch via `rayon::par_iter()`, aggregation (graphs, dead exports)
- **`file.rs`** — per-file analysis: parse AST → extract metrics → return `*FileData`
- **`visitors.rs`** — AST traversal helpers (complexity, nesting, hooks, unsafe, etc.)

### Parsers Used

| Language | Parser | Notes |
|----------|--------|-------|
| Python | `rustpython-parser` 0.4 | Full CPython-compatible AST |
| TypeScript/JS | `oxc_parser` 0.122 | Fastest TS parser (3–5× vs SWC) |
| Rust | `syn` 2 (full + visit) | Token-based, handles all Rust syntax |

### Output Shape

Single language → flat JSON object with keys like `scanner`, `summary`, `complexity`, `cycles`, etc.
Multiple languages → `{ "python": {...}, "typescript": {...}, "rust": {...} }`.

### Threshold / Exit Codes

- **Exit 0** — clean or within thresholds
- **Exit 1** — threshold breach (`--max-complexity`, `--max-nesting`, `--max-cycles`, etc.)
- **Exit 2** — invalid CLI args (e.g. unknown `--skip` section name)

## Test Fixtures

- `fixtures/minimal-py/` — minimal Python package for integration tests
- `fixtures/minimal-ts/` — minimal TypeScript file for integration tests
- Integration tests live in `rust/tests/integration.rs` and invoke the compiled binary end-to-end

## Optional TypeScript Checks

These are off by default and require explicit flags:
- `--orm-check where,andWhere` — detect camelCase column names in ORM calls
- `--boundary "layerA/:@pkg/layerB"` — enforce import boundary rules between layers

## Workspace Layout

Root `Cargo.toml` defines a workspace with a single member `rust/`. The binary crate is at `rust/src/main.rs`. Lock file lives at workspace root.
