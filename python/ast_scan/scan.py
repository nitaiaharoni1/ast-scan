"""AST-based codebase scan for Python packages (complexity, imports, cycles, routes)."""

from __future__ import annotations

import argparse
import ast
import json
import os
import sys
from collections import defaultdict
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

# Sections that can be omitted from the text report via --skip (does not affect --json).
PY_TEXT_SKIP_SECTIONS = frozenset(
    {
        "inventory",
        "complexity",
        "imports",
        "cycles",
        "dead-exports",
        "decorators",
        "routes",
    }
)


# ── Data classes ──────────────────────────────────────────────────────────────


@dataclass
class FuncInfo:
    name: str
    qualname: str
    file: str
    line: int
    end_line: int
    line_count: int
    complexity: int = 1
    decorators: list[str] = field(default_factory=list)
    is_method: bool = False


@dataclass
class ClassInfo:
    name: str
    file: str
    line: int
    end_line: int
    line_count: int
    methods: list[FuncInfo] = field(default_factory=list)
    decorators: list[str] = field(default_factory=list)


@dataclass
class ImportEdge:
    source_module: str
    target_module: str
    names: list[str]


@dataclass
class RouteInfo:
    method: str
    path: str
    handler: str
    file: str
    line: int
    dependencies: list[str] = field(default_factory=list)


# ── Helpers ───────────────────────────────────────────────────────────────────


def display_rel(path: str, scan_root: Path) -> str:
    try:
        return str(Path(path).relative_to(scan_root.parent))
    except ValueError:
        try:
            return str(Path(path).relative_to(Path.cwd()))
        except ValueError:
            return path


def file_to_module(fpath: Path, scan_root: Path, pkg: str) -> str:
    try:
        r = fpath.relative_to(scan_root)
    except ValueError:
        return str(fpath)
    parts = list(r.with_suffix("").parts)
    if parts and parts[-1] == "__init__":
        parts.pop()
    if not parts:
        return pkg
    return f"{pkg}." + ".".join(parts)


def const_value_str(node: ast.expr) -> str:
    if isinstance(node, ast.Constant):
        return repr(node.value)
    return ast.dump(node)


def decorator_name(node: ast.expr) -> str:
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        return f"{decorator_name(node.value)}.{node.attr}"
    if isinstance(node, ast.Call):
        return decorator_name(node.func)
    return ast.dump(node)


def decorator_repr(node: ast.expr) -> str:
    if isinstance(node, ast.Call):
        fname = decorator_name(node.func)
        args: list[str] = []
        for a in node.args:
            args.append(const_value_str(a))
        for kw in node.keywords:
            val = const_value_str(kw.value)
            args.append(f"{kw.arg}={val}")
        return f"@{fname}({', '.join(args)})"
    return f"@{decorator_name(node)}"


# ── Complexity ────────────────────────────────────────────────────────────────


class ComplexityVisitor(ast.NodeVisitor):
    def __init__(self) -> None:
        self.complexity = 1

    def visit_If(self, node: ast.If) -> None:
        self.complexity += 1
        self.generic_visit(node)

    def visit_For(self, node: ast.For) -> None:
        self.complexity += 1
        self.generic_visit(node)

    def visit_While(self, node: ast.While) -> None:
        self.complexity += 1
        self.generic_visit(node)

    def visit_ExceptHandler(self, node: ast.ExceptHandler) -> None:
        self.complexity += 1
        self.generic_visit(node)

    def visit_With(self, node: ast.With) -> None:
        self.complexity += 1
        self.generic_visit(node)

    def visit_Assert(self, node: ast.Assert) -> None:
        self.complexity += 1
        self.generic_visit(node)

    def visit_BoolOp(self, node: ast.BoolOp) -> None:
        self.complexity += len(node.values) - 1
        self.generic_visit(node)

    def visit_comprehension(self, node: ast.comprehension) -> None:
        self.complexity += 1
        self.complexity += len(node.ifs)
        self.generic_visit(node)


def compute_complexity(node: ast.AST) -> int:
    v = ComplexityVisitor()
    v.visit(node)
    return v.complexity


# ── Main collector ────────────────────────────────────────────────────────────


class FileAnalyzer(ast.NodeVisitor):
    def __init__(self, filepath: str, module: str, pkg: str) -> None:
        self.filepath = filepath
        self.module = module
        self._pkg = pkg
        self.functions: list[FuncInfo] = []
        self.classes: list[ClassInfo] = []
        self.imports: list[ImportEdge] = []
        self.exports: list[str] = []
        self.top_level_names: list[str] = []
        self.routes: list[RouteInfo] = []
        self._class_stack: list[str] = []

    def visit_Module(self, node: ast.Module) -> None:
        for item in node.body:
            if isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef)):
                self._process_function(item, is_method=False)
                self.top_level_names.append(item.name)
            elif isinstance(item, ast.ClassDef):
                self._process_class(item)
                self.top_level_names.append(item.name)
            elif isinstance(item, ast.Assign):
                for target in item.targets:
                    if isinstance(target, ast.Name):
                        self.top_level_names.append(target.id)
                        if target.id == "__all__" and isinstance(
                            item.value, (ast.List, ast.Tuple, ast.Set)
                        ):
                            for elt in item.value.elts:
                                if isinstance(elt, ast.Constant) and isinstance(elt.value, str):
                                    self.exports.append(elt.value)
            elif isinstance(item, ast.AnnAssign) and isinstance(item.target, ast.Name):
                self.top_level_names.append(item.target.id)
            elif isinstance(item, (ast.Import, ast.ImportFrom)):
                self._process_import(item)

    def _process_function(
        self,
        node: ast.FunctionDef | ast.AsyncFunctionDef,
        is_method: bool = False,
    ) -> FuncInfo:
        end = node.end_lineno or node.lineno
        qualname = ".".join(self._class_stack + [node.name]) if self._class_stack else node.name
        info = FuncInfo(
            name=node.name,
            qualname=qualname,
            file=self.filepath,
            line=node.lineno,
            end_line=end,
            line_count=end - node.lineno + 1,
            complexity=compute_complexity(node),
            decorators=[decorator_repr(d) for d in node.decorator_list],
            is_method=is_method,
        )
        self.functions.append(info)
        self._extract_route(node, info)
        return info

    def _process_class(self, node: ast.ClassDef) -> None:
        end = node.end_lineno or node.lineno
        cls = ClassInfo(
            name=node.name,
            file=self.filepath,
            line=node.lineno,
            end_line=end,
            line_count=end - node.lineno + 1,
            decorators=[decorator_repr(d) for d in node.decorator_list],
        )
        self._class_stack.append(node.name)
        for item in node.body:
            if isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef)):
                fi = self._process_function(item, is_method=True)
                cls.methods.append(fi)
        self._class_stack.pop()
        self.classes.append(cls)

    def _process_import(self, node: ast.Import | ast.ImportFrom) -> None:
        p = self._pkg
        if isinstance(node, ast.Import):
            for alias in node.names:
                mod = alias.name
                if mod.startswith(p + ".") or mod == p:
                    self.imports.append(ImportEdge(self.module, mod, [alias.asname or alias.name]))
        elif isinstance(node, ast.ImportFrom) and node.module:
            mod = node.module
            if node.level > 0:
                parts = self.module.split(".")
                base = ".".join(parts[: max(1, len(parts) - node.level)])
                mod = f"{base}.{mod}" if mod else base
            if mod.startswith(p + ".") or mod == p:
                names = [a.name for a in node.names]
                self.imports.append(ImportEdge(self.module, mod, names))

    def _extract_route(self, node: ast.FunctionDef | ast.AsyncFunctionDef, info: FuncInfo) -> None:
        http_methods = {"get", "post", "put", "delete", "patch", "head", "options"}
        for dec in node.decorator_list:
            if not isinstance(dec, ast.Call):
                continue
            func = dec.func if isinstance(dec, ast.Call) else dec
            if not isinstance(func, ast.Attribute) or func.attr not in http_methods:
                continue
            method = func.attr.upper()
            path = ""
            if dec.args and isinstance(dec.args[0], ast.Constant):
                path = str(dec.args[0].value)
            deps: list[str] = []
            for arg in ast.walk(node):
                if isinstance(arg, ast.Call) and isinstance(arg.func, ast.Name) and arg.func.id == "Depends":
                    if arg.args:
                        deps.append(ast.dump(arg.args[0]))
            self.routes.append(RouteInfo(method, path, info.qualname, self.filepath, node.lineno, deps))


def find_cycles(graph: dict[str, set[str]]) -> list[list[str]]:
    WHITE, GRAY, BLACK = 0, 1, 2
    color: dict[str, int] = defaultdict(int)
    path: list[str] = []
    cycles: list[list[str]] = []

    def dfs(u: str) -> None:
        color[u] = GRAY
        path.append(u)
        for v in sorted(graph.get(u, set())):
            if color[v] == GRAY:
                idx = path.index(v)
                cycle = path[idx:] + [v]
                cycles.append(cycle)
            elif color[v] == WHITE:
                dfs(v)
        path.pop()
        color[u] = BLACK

    for node in sorted(graph):
        if color[node] == WHITE:
            dfs(node)
    return cycles


def collect_py_files(scan_root: Path) -> list[Path]:
    result: list[Path] = []
    for dirpath, _dirs, filenames in os.walk(scan_root):
        for f in filenames:
            if f.endswith(".py"):
                result.append(Path(dirpath) / f)
    return sorted(result)


def unique_cycles(cycles: list[list[str]]) -> list[list[str]]:
    seen: set[frozenset[str]] = set()
    unique: list[list[str]] = []
    for c in cycles:
        key = frozenset(c[:-1])
        if key not in seen:
            seen.add(key)
            unique.append(c)
    return unique


def analyze_python(
    scan_root: Path,
    pkg: str,
) -> dict[str, Any]:
    scan_root = scan_root.resolve()
    if not scan_root.is_dir():
        raise SystemExit(f"Not a directory: {scan_root}")

    files = collect_py_files(scan_root)
    all_functions: list[FuncInfo] = []
    all_classes: list[ClassInfo] = []
    all_imports: list[ImportEdge] = []
    all_routes: list[RouteInfo] = []
    module_top_names: dict[str, list[str]] = {}
    file_lines: dict[str, int] = {}
    decorator_freq: dict[str, int] = defaultdict(int)

    imported_names: dict[str, set[str]] = defaultdict(set)
    parse_errors: list[dict[str, str]] = []

    for fpath in files:
        module = file_to_module(fpath, scan_root, pkg)
        source = fpath.read_text(encoding="utf-8", errors="replace")
        file_lines[display_rel(str(fpath), scan_root)] = source.count("\n") + 1
        try:
            tree = ast.parse(source, filename=str(fpath))
        except SyntaxError as e:
            parse_errors.append({"file": display_rel(str(fpath), scan_root), "message": str(e)})
            continue

        analyzer = FileAnalyzer(str(fpath), module, pkg)
        analyzer.visit_Module(tree)

        all_functions.extend(analyzer.functions)
        all_classes.extend(analyzer.classes)
        all_imports.extend(analyzer.imports)
        all_routes.extend(analyzer.routes)
        module_top_names[module] = analyzer.top_level_names

        for edge in analyzer.imports:
            for name in edge.names:
                imported_names[edge.target_module].add(name)

        for fn in analyzer.functions:
            for d in fn.decorators:
                dname = d.split("(")[0].lstrip("@")
                decorator_freq[dname] += 1
        for cls in analyzer.classes:
            for d in cls.decorators:
                dname = d.split("(")[0].lstrip("@")
                decorator_freq[dname] += 1

    graph: dict[str, set[str]] = defaultdict(set)
    for edge in all_imports:
        graph[edge.source_module].add(edge.target_module)

    all_modules = set(module_top_names.keys())
    cycles = find_cycles(graph)
    unique = unique_cycles(cycles)

    skip_private = {"__init__", "__all__", "__version__"}
    dead: list[dict[str, str]] = []
    for mod, names in module_top_names.items():
        used = imported_names.get(mod, set())
        for name in names:
            if name.startswith("_") or name in skip_private:
                continue
            if name not in used:
                dead.append({"module": mod, "name": name})
    dead.sort(key=lambda x: (x["module"], x["name"]))

    in_degree: dict[str, int] = defaultdict(int)
    for edge in all_imports:
        in_degree[edge.target_module] += 1

    total_lines = sum(file_lines.values())
    total_edges = sum(len(v) for v in graph.values())

    complexity_rows = [
        {
            "name": fn.qualname,
            "cc": fn.complexity,
            "file": display_rel(fn.file, scan_root),
            "line": fn.line,
            "is_method": fn.is_method,
        }
        for fn in sorted(all_functions, key=lambda f: -f.complexity)
    ]

    return {
        "scanner": "python",
        "scan_root": str(scan_root),
        "package": pkg,
        "summary": {
            "files": len(files),
            "files_parsed": len(files) - len(parse_errors),
            "parse_errors": len(parse_errors),
            "lines": total_lines,
            "functions": len(all_functions),
            "classes": len(all_classes),
            "internal_imports": len(all_imports),
        },
        "parse_errors": parse_errors,
        "inventory": {
            "files_by_lines": [
                {"file": fp, "lines": lc}
                for fp, lc in sorted(file_lines.items(), key=lambda x: -x[1])
            ],
            "largest_functions": [
                {
                    "name": fn.qualname,
                    "lines": fn.line_count,
                    "file": display_rel(fn.file, scan_root),
                    "line": fn.line,
                    "is_method": fn.is_method,
                }
                for fn in sorted(all_functions, key=lambda f: -f.line_count)
            ],
            "largest_classes": [
                {
                    "name": cls.name,
                    "lines": cls.line_count,
                    "methods": len(cls.methods),
                    "file": display_rel(str(cls.file), scan_root),
                    "line": cls.line,
                }
                for cls in sorted(all_classes, key=lambda c: -c.line_count)
            ],
        },
        "complexity": complexity_rows,
        "imports": {
            "modules": len(all_modules),
            "edges": total_edges,
            "top_imported": [
                {"module": mod, "count": cnt}
                for mod, cnt in sorted(in_degree.items(), key=lambda x: -x[1])
            ],
        },
        "cycles": [" -> ".join(c) for c in unique],
        "cycles_raw": unique,
        "dead_exports": dead,
        "decorators": [
            {"decorator": dname, "count": cnt}
            for dname, cnt in sorted(decorator_freq.items(), key=lambda x: -x[1])
        ],
        "routes": [
            {
                "method": r.method,
                "path": r.path,
                "handler": r.handler,
                "file": display_rel(r.file, scan_root),
                "line": r.line,
                "dependencies": r.dependencies,
            }
            for r in sorted(all_routes, key=lambda r: (r.path, r.method))
        ],
    }


def print_text_report(data: dict[str, Any], title: str, top: int, skip: set[str]) -> None:
    def sep() -> None:
        print("=" * 72)

    sep()
    print(f"  {title}")
    sep()
    print()
    s = data["summary"]
    print(f"  Files analyzed:   {s['files']}")
    if s.get("parse_errors"):
        print(f"  Parse errors:     {s['parse_errors']}")
    print(f"  Total lines:      {s['lines']:,}")
    print(f"  Functions:        {s['functions']}")
    print(f"  Classes:          {s['classes']}")
    print(f"  Internal imports: {s['internal_imports']}")
    print()

    if "inventory" not in skip:
        inv = data["inventory"]
        sep()
        print(f"  1. INVENTORY — Top {top} Largest Files")
        sep()
        for row in inv["files_by_lines"][:top]:
            print(f"  {row['lines']:>5} lines  {row['file']}")
        print()
        print(f"  Top {top} Largest Functions/Methods")
        print("  " + "-" * 60)
        for row in inv["largest_functions"][:top]:
            tag = " (method)" if row["is_method"] else ""
            print(f"  {row['lines']:>5} lines  {row['name']}{tag}  [{row['file']}:{row['line']}]")
        print()
        print(f"  Top {top} Largest Classes")
        print("  " + "-" * 60)
        for row in inv["largest_classes"][:top]:
            print(
                f"  {row['lines']:>5} lines  {row['name']}  ({row['methods']} methods)  "
                f"[{row['file']}:{row['line']}]"
            )
        print()

    cc_top = max(top, 30)
    if "complexity" not in skip:
        sep()
        print(f"  2. CYCLOMATIC COMPLEXITY — Top {cc_top}")
        sep()
        for row in data["complexity"][:cc_top]:
            tag = " (method)" if row["is_method"] else ""
            print(f"  CC={row['cc']:>3}  {row['name']}{tag}  [{row['file']}:{row['line']}]")
        print()

    if "imports" not in skip:
        imp = data["imports"]
        sep()
        print("  3. IMPORT DEPENDENCY GRAPH")
        sep()
        print(f"  Internal modules:      {imp['modules']}")
        print(f"  Internal import edges: {imp['edges']}")
        print()
        print(f"  Top {top} Most-Imported Internal Modules")
        print("  " + "-" * 60)
        for row in imp["top_imported"][:top]:
            print(f"  {row['count']:>4}x  {row['module']}")
        print()

    if "cycles" not in skip:
        sep()
        print("  4. CIRCULAR IMPORTS")
        sep()
        raw: list[list[str]] = data["cycles_raw"]
        if raw:
            print(f"  Found {len(raw)} cycle(s):")
            for i, c in enumerate(raw, 1):
                print(f"  [{i}] {' -> '.join(c)}")
        else:
            print("  None found.")
        print()

    if "dead-exports" not in skip:
        sep()
        print("  5. DEAD EXPORTS (exported but never imported internally)")
        sep()
        dead: list[dict[str, str]] = data["dead_exports"]
        if dead:
            cap = max(80, top * 4)
            print(f"  Found {len(dead)} potentially dead export(s):")
            for d in dead[:cap]:
                print(f"    {d['module']}.{d['name']}")
            if len(dead) > cap:
                print(f"    ... and {len(dead) - cap} more")
        else:
            print("  None found.")
        print()

    if "decorators" not in skip:
        sep()
        print("  6. DECORATOR AUDIT")
        sep()
        print()
        print("  Frequency Table")
        print("  " + "-" * 60)
        for row in data["decorators"][:top]:
            print(f"  {row['count']:>4}x  {row['decorator']}")
        print()

    if "routes" not in skip:
        sep()
        print("  7. API ROUTE INVENTORY (FastAPI-style)")
        sep()
        routes = data["routes"]
        if routes:
            print(f"  {'METHOD':<8} {'PATH':<40} {'HANDLER':<30} {'FILE'}")
            print("  " + "-" * 110)
            for r in routes:
                rf = r["file"]
                deps_str = f"  deps: {', '.join(r['dependencies'])}" if r.get("dependencies") else ""
                print(f"  {r['method']:<8} {r['path']:<40} {r['handler']:<30} {rf}:{r['line']}{deps_str}")
        else:
            print("  No routes found.")
        print()

    if data.get("parse_errors"):
        sep()
        print("  PARSE ERRORS (skipped)")
        sep()
        for e in data["parse_errors"]:
            print(f"    {e['file']}: {e['message']}")
        print()

    sep()
    print("  END OF REPORT")
    sep()


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    p = argparse.ArgumentParser(
        prog="ast-scan",
        description="AST-based health scan for a Python package directory.",
    )
    p.add_argument(
        "path",
        type=Path,
        help="Root directory to scan (all .py files under this path, e.g. src/myapp)",
    )
    p.add_argument(
        "--pkg",
        default=None,
        help="Top-level package name for internal import detection (default: last segment of path)",
    )
    p.add_argument(
        "--title",
        default=None,
        help="Report title (default: derived from path)",
    )
    p.add_argument("--top", type=int, default=20, help="How many items to show in ranked sections")
    p.add_argument(
        "--json",
        action="store_true",
        help="Emit full JSON report (all sections; --skip applies only to text output)",
    )
    p.add_argument(
        "--skip",
        action="append",
        default=[],
        metavar="SECTION",
        help="Omit SECTION from text report only (repeatable): inventory, complexity, "
        "imports, cycles, dead-exports, decorators, routes",
    )
    return p.parse_args(argv)


def main(argv: list[str] | None = None) -> None:
    args = parse_args(argv)
    scan_root: Path = args.path
    pkg = args.pkg or scan_root.resolve().name
    title = args.title or f"{pkg.upper()} — AST ANALYSIS (Python)"
    skip = set(args.skip)
    unknown = skip - PY_TEXT_SKIP_SECTIONS
    if unknown:
        bad = ", ".join(sorted(unknown))
        print(
            f"ast-scan: unknown --skip section(s): {bad}. "
            f"Valid: {', '.join(sorted(PY_TEXT_SKIP_SECTIONS))}",
            file=sys.stderr,
        )
        raise SystemExit(2)

    data = analyze_python(scan_root, pkg)
    data["title"] = title

    if args.json:
        # JSON-friendly: drop cycles_raw duplicate if desired; keep both for programs
        out = {k: v for k, v in data.items() if k != "title"}
        out["report_title"] = title
        print(json.dumps(out, indent=2))
        return

    print_text_report(data, title, args.top, skip)


if __name__ == "__main__":
    main()
