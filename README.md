# ast-scan

**AST-based codebase health scanner** for Python and TypeScript/JavaScript projects. One pass reports cyclomatic complexity hotspots, import graphs, circular dependencies, potentially dead exports, and (where applicable) FastAPI routes or React hook/component patterns.

This repository ships **two installable CLIs** with the same command name (`ast-scan`): one for Python packages (PyPI-ready) and one for TS/JS (npm-ready). They target different ecosystems; install only the one you need.

## Features

| Area | Python CLI | TypeScript CLI |
|------|------------|----------------|
| Cyclomatic complexity (top symbols) | yes | yes |
| Largest files / functions / classes | yes | files / functions |
| Internal import graph + in-degree | yes | yes |
| Circular import detection | yes | yes |
| Dead exports (heuristic) | yes | yes |
| Decorator frequency | yes | — |
| FastAPI-style route inventory | yes | — |
| React components, props, hooks | — | yes |
| External package frequency | — | yes |

**Heuristic warnings:** “Dead exports” ignores private names and entry-like files but still produces false positives (e.g. symbols only used via dynamic imports, framework entrypoints, or re-exports consumed outside the scanned tree).

## Installation

### Python (scan Python packages)

From the `python/` directory:

```bash
cd python
pip install .
# or editable:
pip install -e .
```

The `ast-scan` executable is added to your environment. You can also run:

```bash
python -m ast_scan <path> [options]
```

### TypeScript / JavaScript

From the `typescript/` directory:

```bash
cd typescript
npm install
npm run build
npm link   # optional: global ast-scan
```

Or run without linking:

```bash
npx tsx src/scan.ts <path> [options]
# after build:
node dist/scan.js <path> [options]
```

## Quick start

**Python** — point at the package root (the directory containing your top-level package, e.g. `src/myapp`):

```bash
ast-scan ./src/myapp --pkg myapp
# --pkg defaults to the last path segment if omitted
```

**TypeScript** — point at `src/` (or any tree of `.ts`/`.tsx` files):

```bash
ast-scan ./src --alias @/
# --alias defaults to @/; use e.g. ~/ for other path aliases
```

### JSON output (CI / dashboards)

```bash
ast-scan ./src/myapp --pkg myapp --json > report.json
ast-scan ./src --json > report-frontend.json
```

### Skip sections

Repeat `--skip` to omit parts of the text report:

**Python:** `inventory`, `complexity`, `imports`, `cycles`, `dead-exports`, `decorators`, `routes`

**TypeScript:** `inventory`, `complexity`, `imports`, `cycles`, `dead-exports`, `component-props`, `hooks`

```bash
ast-scan ./src --skip inventory --skip complexity
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

JSON shape (illustrative):

```json
{
  "scanner": "python",
  "package": "myapp",
  "summary": { "files": 120, "lines": 45000, "functions": 800 },
  "complexity": [{ "name": "run", "cc": 35, "file": "myapp/runner.py", "line": 10 }],
  "imports": { "modules": 90, "edges": 400, "top_imported": [{ "module": "myapp.core", "count": 50 }] },
  "cycles": ["a.b -> a.c -> a.b"],
  "dead_exports": [{ "module": "myapp.util", "name": "helper" }]
}
```

## Section reference

1. **Inventory** — Large files and symbols; good refactor candidates by size.
2. **Cyclomatic complexity** — Branching density; high values imply harder testing and more bug risk.
3. **Import graph** — Which internal modules are depended on most (high in-degree = load-bearing).
4. **Circular imports** — Cycles in the internal graph; can cause init-order issues.
5. **Dead exports** — Symbols exported from a module but never imported by name elsewhere in the scan (heuristic).
6. **Decorators (Python)** — Frequency of decorator usage across functions/classes.
7. **Routes (Python)** — HTTP method + path + handler for FastAPI-style `@router.get/post/...` calls.
8. **Component props / hooks (TS)** — React-oriented structure and hook usage patterns.

## How this relates to other tools

| Tool | Role |
|------|------|
| [radon](https://pypi.org/project/radon/) | Python complexity & maintainability index |
| [ruff](https://docs.astral.sh/ruff/) | Linting; rule `C901` flags overly complex functions |
| [knip](https://knip.dev/) | Unused files/exports/deps in JS/TS |
| [madge](https://www.npmjs.com/package/madge) | Dependency graphs and circular deps for JS |
| **ast-scan** | **Single combined report** plus **FastAPI route** and **React hook** insights; zero extra Python deps |

Use dedicated linters and dead-code tools for enforcement in CI; use **ast-scan** for a quick structural overview or to generate JSON for custom dashboards.

## Contributing

Issues and PRs welcome. When changing scanners, run:

```bash
cd python && python -m ast_scan /path/to/package --pkg pkgname
cd typescript && npm run build && node dist/scan.js /path/to/src
```

## License

MIT — see [LICENSE](LICENSE).
