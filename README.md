# ast-scan

**AST-based codebase health scanner** for Python, TypeScript/JavaScript, and **Rust** projects. A single **Rust** binary reports cyclomatic complexity, import graphs, circular dependencies, potentially dead exports, and (by mode) FastAPI-style routes, React hook/component patterns, or Rust-specific audits (`unsafe`, `unwrap`/`expect`, `#[allow]`, derives).

The CLI **auto-detects** which languages are present (`.py`, `.ts`/`.js`, `.rs`) and runs **every** matching scanner in a fixed order (Python, then TypeScript, then Rust). Mixed trees get one combined text report (sections back-to-back). You can **force** one or more scanners with `--python` / `--typescript` / `--rust` (flags can be combined, e.g. `--python --rust` only).

## Features

| Area | Python | TypeScript / JS | Rust |
|------|--------|-----------------|------|
| Cyclomatic complexity (top symbols) | yes | yes | yes |
| Nesting depth (top symbols) | yes | yes | yes |
| Largest files / symbols | yes | yes | yes (structs/enums) |
| Internal import graph + in-degree | yes | yes | yes (`crate::` / `super::` / `self::`) |
| Module coupling (Ca / Ce / Instability) | yes | yes | yes |
| Circular import detection | yes | yes | yes |
| Dead exports (heuristic) | yes | yes (re-export aware) | yes (`pub` + internal `use`) |
| TODO / FIXME / HACK comment audit | yes | yes | yes |
| Silent exception / catch handlers | yes | yes | — |
| Decorator frequency | yes | — | — |
| FastAPI-style route inventory | yes | — | — |
| React components, props, hooks | — | yes | — |
| External package / crate frequency | — | yes | yes |
| ESLint disable comment audit | — | yes | — |
| `@ts-ignore` / `@ts-expect-error` audit | — | yes | — |
| Explicit `any` type audit | — | yes | — |
| `console.log` / `debugger` audit | — | yes | — |
| MobX missing `observer()` detection | — | yes | — |
| ORM case convention check (opt-in) | — | yes | — |
| Import boundary enforcement (opt-in) | — | yes | — |
| `.js` / `.jsx` file support | — | yes | — |
| Unsafe + `unwrap`/`expect` audit | — | — | yes |
| `#[allow(...)]` + derive macro audit | — | — | yes |
| Trait inventory | — | — | yes |
| `--exclude` path filtering | yes | yes | yes |
| CI threshold exit codes | yes | yes | yes |

**Heuristic warnings:** “Dead exports” ignores private names and entry-like files but still produces false positives (e.g. symbols only used via dynamic imports, framework entrypoints, or re-exports consumed outside the scanned tree).

## Installation

From the `rust/` directory:

```bash
cd rust
cargo build --release
# binary: target/release/ast-scan
```

Install into your cargo bin path:

```bash
cd rust
cargo install --path .
```

Then run `ast-scan` from your `PATH`.

## Quick start

**Python** — point at the package root and set the top-level package name (defaults to the last segment of the resolved path):

```bash
ast-scan ./src/myapp --pkg myapp
# force only Python when multiple languages exist:
ast-scan ./src/myapp --python --pkg myapp
# monorepo with .py + .ts + .rs — no flags runs all scanners:
ast-scan .
```

**TypeScript / JavaScript** — point at `src/` (or any tree of `.ts`/`.tsx`/`.js`/`.jsx` files):

```bash
ast-scan ./src --typescript --alias @/
# --alias defaults to @/; use e.g. ~/ for other path aliases
```

**Rust** — point at a crate root or `src/` (any tree of `.rs` files; `target/` is skipped):

```bash
ast-scan ./rust/src --rust
# or from repo root if only .rs files are present:
ast-scan . --rust
```

### JSON output (CI / dashboards)

```bash
ast-scan ./src/myapp --pkg myapp --json > report.json
ast-scan ./src --typescript --json > report-frontend.json
ast-scan ./rust/src --rust --json > report-rust.json
# multiple languages: top-level keys per scanner + report_title, e.g. "python", "typescript", "rust"
ast-scan . --pkg myorg --json > report-monorepo.json
```

### Skip sections (text report only)

Repeat `--skip` to omit parts of the **text** report. **`--json` always returns the full structure** (all sections); `--skip` does not trim JSON.

When more than one scanner runs, `--skip` names must be valid for **at least one** of the active modes (the allowed set is the union of that mode’s sections).

Unknown section names exit with code 2 and print the allowed list.

**Python text sections:** `inventory`, `complexity`, `nesting`, `imports`, `coupling`, `cycles`, `dead-exports`, `silent-except`, `todo-audit`, `decorators`, `routes`

**TypeScript text sections:** `inventory`, `complexity`, `nesting`, `imports`, `coupling`, `cycles`, `dead-exports`, `component-props`, `hooks`, `console-debugger`, `silent-catches`, `eslint-disables`, `any-audit`, `ts-directives`, `todo-audit`, `mobx-observer`, `orm-case-check`, `import-boundaries`

**Rust text sections:** `inventory`, `complexity`, `nesting`, `imports`, `coupling`, `cycles`, `dead-exports`, `unsafe-audit`, `unwrap-audit`, `allow-lints`, `derive-audit`, `traits`, `todo-audit`, `parse-errors`

```bash
ast-scan ./src --typescript --skip inventory --skip complexity
ast-scan ./rust/src --rust --skip derive-audit
```

### Exclude paths

Repeat `--exclude` to skip directories or files matching a prefix or substring (relative to the scan root).

```bash
ast-scan ./src --exclude generated --exclude __pycache__ --exclude vendor
```

### CI threshold exit codes

Use `--max-complexity`, `--max-nesting`, and `--max-cycles` to gate CI pipelines. Exit code **1** is returned if any threshold is breached (text mode). In TypeScript text mode, import-boundary violations also set exit code **1**. Rust mode uses the same `--max-*` flags against the Rust complexity / nesting / cycle JSON sections. With multiple scanners, threshold messages are prefixed with `[python]`, `[typescript]`, or `[rust]`.

```bash
ast-scan ./src --max-complexity 25 --max-nesting 5 --max-cycles 0
```

### ORM case convention check (TypeScript, opt-in)

Flag camelCase identifiers inside string arguments to ORM / query-builder method calls.

```bash
# TypeORM-style
ast-scan ./backend/src --typescript --orm-check where,andWhere,orWhere,orderBy,addOrderBy,select,addSelect,groupBy,addGroupBy,having,andHaving,orHaving

# Knex-style
ast-scan ./src --typescript --orm-check where,orWhere,orderBy,select,groupBy,having
```

### Import boundary enforcement (TypeScript, opt-in)

Each `--boundary` flag defines a rule: files whose path starts with `source_prefix` must not import modules whose resolved path starts with a forbidden prefix. Repeatable.

```bash
ast-scan ./src --typescript \
  --boundary "shared-utils/:@myorg/shared-stores,@myorg/shared-ui" \
  --boundary "shared-stores/:@myorg/shared-ui" \
  --boundary "app-a/:app-b/"
```

## Sample output (truncated)

```
========================================================================
  MYAPP — AST ANALYSIS (Python)
========================================================================

  Files analyzed:   120
  Total lines:      45,000
  ...

========================================================================
  2. CYCLOMATIC COMPLEXITY — Top 30
========================================================================
  CC= 42  process_payment (method)  [myapp/billing/service.py:88]
  ...
```

JSON shape for a **single** scanner (illustrative — includes `report_title`; each scanner also emits a `scanner` field such as `"python"`, `"typescript"`, or `"rust"`):

```json
{
  "report_title": "MYAPP — AST ANALYSIS (Python)",
  "scanner": "python",
  "package": "myapp",
  "summary": { "files": 120, "lines": 45000, "functions": 800 },
  "complexity": [{ "name": "run", "cc": 35, "file": "myapp/runner.py", "line": 10 }],
  "imports": { "modules": 90, "edges": 400, "top_imported": [{ "module": "myapp.core", "count": 50 }] },
  "cycles": ["a.b -> a.c -> a.b"],
  "dead_exports": [{ "module": "myapp.util", "name": "helper" }]
}
```

When **multiple** scanners run with `--json`, the root object has `report_title` plus one key per language (`python`, `typescript`, `rust`), each holding that scanner’s payload (without nested `report_title`).

## Section reference

1. **Inventory** — Large files, functions, and classes; good refactor candidates by size.
2. **Cyclomatic complexity** — Branching density; high values imply harder testing and more bug risk.
2b. **Nesting depth** — Maximum control-flow nesting per function; complementary to CC for readability.
3. **Import graph** — Which internal modules are depended on most (high in-degree = load-bearing).
3b. **Module coupling** — Afferent (Ca), efferent (Ce), and instability (I = Ce / (Ca + Ce)) per module.
4. **Circular imports** — Cycles in the internal graph; can cause init-order issues.
5. **Dead exports** — Symbols exported from a module but never imported by name elsewhere in the scan (heuristic). TS mode tracks re-exports to reduce false positives.
6. **TODO / FIXME / HACK comments** — Frequency of tech-debt markers with sample locations.
7. **Silent exception handlers** — Python: `except: pass` and similar; TS: empty `catch {}` and trivial `.catch(() => {})`.
7b. **Console / debugger audit (TS)** — Leftover `console.log`, `console.error`, `debugger` statements.
8. **Decorators (Python)** — Frequency of decorator usage across functions/classes.
9. **Routes (Python)** — HTTP method + path + handler for FastAPI-style `@router.get/post/...` calls.
10. **Component props / hooks (TS)** — React-oriented structure and hook usage patterns.
11. **ESLint disable audit (TS)** — Aggregates `eslint-disable` comments by rule name with sample files.
11b. **Explicit `any` type audit (TS)** — Counts `any` usage per file.
12. **TS directive audit (TS)** — `@ts-ignore`, `@ts-expect-error`, `@ts-nocheck`.
13. **MobX observer (TS)** — Exported React components not wrapped in `observer()` when MobX imports are detected.
14. **ORM case check (TS, opt-in)** — `--orm-check METHOD1,METHOD2,...`.
15. **Import boundaries (TS, opt-in)** — `--boundary source:forbidden1,forbidden2`; exit **1** on violations in text mode.
16. **Unsafe / unwrap audit (Rust)** — Counts `unsafe` functions/blocks and `.unwrap()` / `.expect()` call sites (heuristic).
17. **`#[allow]` / derive audit (Rust)** — Frequency of lint suppressions and `#[derive(...)]` macros.
18. **Trait inventory (Rust)** — `pub` and private `trait` declarations with locations.
19. **Parse errors (Rust)** — Files that failed `syn` parse (shown unless `--skip parse-errors`).

## How this relates to other tools

| Tool | Role |
|------|------|
| [radon](https://pypi.org/project/radon/) | Python complexity & maintainability index |
| [ruff](https://docs.astral.sh/ruff/) | Linting; rule `C901` flags overly complex functions |
| [knip](https://knip.dev/) | Unused files/exports/deps in JS/TS |
| [madge](https://www.npmjs.com/package/madge) | Dependency graphs and circular deps for JS |
| [eslint-plugin-mobx](https://www.npmjs.com/package/eslint-plugin-mobx) | ESLint `missing-observer` |
| [cargo-modules](https://github.com/regexident/cargo-modules) / [madge](https://www.npmjs.com/package/madge)-style | Rust / JS dependency structure |
| **ast-scan** | **Single combined report** for Python, TS/JS, and Rust: nesting, coupling, TODO audit, mode-specific checks (routes, React, `unsafe`/unwrap, etc.); **one binary** |

Use dedicated linters and dead-code tools for enforcement in CI; use **ast-scan** for a quick structural overview or JSON for dashboards.

## Contributing

Issues and PRs welcome.

1. Run the binary against a real tree.
2. From `rust/`: `cargo test` and `cargo clippy -- -D warnings`.
3. If you change CLI flags or JSON shape, update this README.

## License

MIT — see [LICENSE](LICENSE).
