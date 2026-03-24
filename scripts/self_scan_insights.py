#!/usr/bin/env python3
"""Regenerate SCAN_INSIGHTS.md — human-readable repository digest from ast-scan --json."""
from __future__ import annotations

import json
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

# Caps keep the markdown readable; raise if you want longer dumps.
TOP_CC = 25
TOP_NEST = 25
TOP_FILES = 30
TOP_FUNCS = 25
TOP_TYPES = 15
TOP_IMPORTED = 25
TOP_EXTERNAL = 25
TOP_COUPLING = 25
DEAD_EXPORTS_MAX = 50
CYCLES_MAX = 60
PARSE_ERRORS_MAX = 20
DECORATORS_MAX = 30
ROUTES_MAX = 40
SILENT_MAX = 30
TRAITS_MAX = 100
UNWRAP_FILES_MAX = 30
ALLOW_RULES_MAX = 30
DERIVE_MAX = 25
TODO_TAGS_MAX = 20
ANY_FILES_MAX = 15
ESLINT_RULES_MAX = 25
CONSOLE_ITEMS_MAX = 20


def h3(lines: list[str], title: str) -> None:
    lines.append(f"### {title}")
    lines.append("")


def md_parse_errors(lines: list[str], pe: Any) -> None:
    if not isinstance(pe, list):
        return
    h3(lines, "Parse errors")
    lines.append(f"- **Count:** {len(pe)}")
    if not pe:
        lines.append("")
        return
    lines.append("- **Sample:**")
    for err in pe[:PARSE_ERRORS_MAX]:
        if isinstance(err, dict):
            msg = (err.get("message") or err.get("error") or "").strip().replace("\n", " ")
            if len(msg) > 160:
                msg = msg[:157] + "…"
            lines.append(f"  - `{err.get('file', '?')}` — {msg}")
        else:
            lines.append(f"  - {err}")
    if len(pe) > PARSE_ERRORS_MAX:
        lines.append(f"  - … and {len(pe) - PARSE_ERRORS_MAX} more")
    lines.append("")


def md_todo_audit(lines: list[str], todo: Any) -> None:
    if not isinstance(todo, dict):
        return
    h3(lines, "TODO / FIXME audit")
    lines.append(f"- **Total markers:** {todo.get('total', 0)}")
    by_tag = todo.get("by_tag") or []
    if by_tag:
        lines.append("- **By tag:**")
        for row in by_tag[:TODO_TAGS_MAX]:
            if isinstance(row, dict):
                lines.append(f"  - **{row.get('tag', '?')}:** {row.get('count', 0)}")
                for loc in (row.get("samples") or [])[:8]:
                    lines.append(f"    - `{loc}`")
            else:
                lines.append(f"  - {row}")
    lines.append("")


def md_imports(
    lines: list[str],
    imp: Any,
    label: str,
    *,
    footnote: str | None = None,
) -> None:
    if not isinstance(imp, dict):
        return
    h3(lines, f"Imports ({label})")
    lines.append(f"- **Internal modules:** {imp.get('modules', '—')}")
    lines.append(f"- **Internal edges:** {imp.get('edges', '—')}")
    ti = imp.get("top_imported") or []
    if ti:
        lines.append(f"- **Most-referenced internal targets (up to {TOP_IMPORTED}):**")
        for row in ti[:TOP_IMPORTED]:
            if isinstance(row, dict):
                lines.append(
                    f"  - {row.get('count', 0)}× `{row.get('module', row.get('name', '?'))}`"
                )
            else:
                lines.append(f"  - {row}")
    if footnote:
        lines.append("")
        lines.append(f"_{footnote}_")
    ext = imp.get("external_crates") or imp.get("external_packages") or []
    if ext:
        lines.append(f"- **External packages / crates (up to {TOP_EXTERNAL}):**")
        for row in ext[:TOP_EXTERNAL]:
            if isinstance(row, dict):
                k = row.get("crate") or row.get("package") or "?"
                lines.append(f"  - {row.get('count', 0)}× `{k}`")
            else:
                lines.append(f"  - {row}")
    lines.append("")


def md_coupling(lines: list[str], rows: Any) -> None:
    h3(lines, "Module coupling (Ca / Ce / instability)")
    lines.append(
        "_**Ca** = inward dependencies; **Ce** = outward; **instability** = Ce / (Ca + Ce) "
        "(0 = stable hub, 1 = leaf)._"
    )
    lines.append("")
    if not isinstance(rows, list) or not rows:
        lines.append(
            "- _No rows: internal graph has no edges, or a single module — typical for tiny trees._"
        )
        lines.append("")
        return
    ranked = sorted(
        rows,
        key=lambda r: (
            (r.get("ca", 0) or 0) + (r.get("ce", 0) or 0),
            r.get("instability") or 0,
        ),
        reverse=True,
    )
    lines.append(f"- **Modules with edges:** {len(rows)} (showing top {TOP_COUPLING} by Ca+Ce)")
    lines.append("")
    lines.append("| Module | Ca | Ce | Instability |")
    lines.append("|--------|----|----|---------------|")
    for r in ranked[:TOP_COUPLING]:
        lines.append(
            f"| `{r.get('module', '')}` | {r.get('ca', 0)} | {r.get('ce', 0)} | {r.get('instability', 0)} |"
        )
    lines.append("")


def md_cycles(lines: list[str], raw: Any) -> None:
    h3(lines, "Circular imports (raw)")
    if not isinstance(raw, list):
        lines.append("- *(none)*")
        lines.append("")
        return
    lines.append(f"- **Count:** {len(raw)}")
    if not raw:
        lines.append("")
        return
    show = raw[:CYCLES_MAX]
    for c in show:
        lines.append(f"  - {c}")
    if len(raw) > CYCLES_MAX:
        lines.append(f"  - … and {len(raw) - CYCLES_MAX} more")
    lines.append("")


def md_dead_exports(lines: list[str], de: Any) -> None:
    h3(lines, "Dead exports (heuristic)")
    lines.append(
        "_`pub` items never referenced by another scanned file’s internal imports — "
        "expect false positives (tests, macros, FFI, dynamic use)._"
    )
    lines.append("")
    if not isinstance(de, list):
        lines.append("")
        return
    lines.append(f"- **Count:** {len(de)}")
    for row in de[:DEAD_EXPORTS_MAX]:
        if isinstance(row, dict):
            lines.append(f"  - `{row.get('module', '')}` :: `{row.get('name', '')}`")
        else:
            lines.append(f"  - {row}")
    if len(de) > DEAD_EXPORTS_MAX:
        lines.append(f"  - … and {len(de) - DEAD_EXPORTS_MAX} more")
    lines.append("")


def md_complexity(lines: list[str], cc: Any, lang: str) -> None:
    h3(lines, "Cyclomatic complexity (ranked)")
    lines.append(
        "_Branching-style proxy (higher = more paths to test). Same idea across Python, TS, and Rust scanners._"
    )
    lines.append("")
    if not isinstance(cc, list) or not cc:
        lines.append("- *(none)*")
        lines.append("")
        return
    top = sorted(cc, key=lambda x: -x.get("cc", 0))[:TOP_CC]
    for x in top:
        extra = []
        if x.get("is_unsafe"):
            extra.append("unsafe")
        if x.get("is_method"):
            extra.append("method")
        if x.get("is_component"):
            extra.append("component")
        suf = f" ({', '.join(extra)})" if extra else ""
        lines.append(
            f"- **CC={x.get('cc')}**{suf} — `{x.get('name', '')}` — `{x.get('file', '')}:{x.get('line', '')}`"
        )
    if len(cc) > TOP_CC:
        lines.append(f"- … _{len(cc) - TOP_CC} more symbols not shown_")
    lines.append("")


def md_nesting(lines: list[str], nest: Any) -> None:
    h3(lines, "Nesting depth (ranked)")
    lines.append("_Deepest control-flow nesting per symbol (complements cyclomatic complexity)._")
    lines.append("")
    if not isinstance(nest, list) or not nest:
        lines.append("- *(none)*")
        lines.append("")
        return
    top = sorted(nest, key=lambda x: -x.get("depth", 0))[:TOP_NEST]
    for x in top:
        lines.append(
            f"- **depth={x.get('depth')}** — `{x.get('name', '')}` — `{x.get('file', '')}:{x.get('line', '')}`"
        )
    if len(nest) > TOP_NEST:
        lines.append(f"- … _{len(nest) - TOP_NEST} more symbols not shown_")
    lines.append("")


def divider(lines: list[str]) -> None:
    lines.append("---")
    lines.append("")


def table_of_contents(lines: list[str]) -> None:
    lines.append("## Table of contents")
    lines.append("")
    for text, anchor in [
        ("Executive summary", "executive-summary"),
        ("How to read this report", "how-to-read-this-report"),
        ("Python", "python"),
        ("TypeScript and JavaScript", "typescript-and-javascript"),
        ("Rust", "rust"),
        ("Regenerate", "regenerate"),
    ]:
        lines.append(f"- [{text}](#{anchor})")
    lines.append("")


def how_to_read(lines: list[str]) -> None:
    lines.append("## How to read this report")
    lines.append("")
    lines.append(
        "- **Source of truth:** `.scan-report.json` from `ast-scan --json`; this Markdown file is a **curated digest**."
    )
    lines.append(
        f"- **Caps:** long ranked lists are truncated (tune `TOP_*` in `scripts/self_scan_insights.py`)."
    )
    lines.append(
        "- **Python scope:** `scripts/` is excluded so this generator’s `.py` file is not part of the Python metrics."
    )
    lines.append(
        "- **Dead exports:** heuristic only; large counts are common for crates with `pub` helpers and re-exports."
    )
    lines.append(
        "- **Complexity (CC) and nesting:** use together with file size when choosing refactor targets."
    )
    lines.append("")


def _n_files(n: int) -> str:
    return f"{n} file" if n == 1 else f"{n} files"


def executive_summary(lines: list[str], data: dict[str, Any]) -> None:
    lines.append("## Executive summary")
    lines.append("")
    rs = data.get("rust") or {}
    py = data.get("python") or {}
    ts = data.get("typescript") or {}
    rs_s = rs.get("summary") or {}
    py_s = py.get("summary") or {}
    ts_s = ts.get("summary") or {}

    lines.append("### Scope")
    lines.append("")
    lines.append(
        f"- **Rust:** {_n_files(int(rs_s.get('files', 0) or 0))}, **{rs_s.get('lines', 0):,}** lines — the real `ast-scan` codebase."
    )
    lines.append(
        f"- **Python:** {_n_files(int(py_s.get('files', 0) or 0))} (fixtures under `fixtures/minimal-py/`; `scripts/` excluded)."
    )
    lines.append(
        f"- **TypeScript:** {_n_files(int(ts_s.get('files', 0) or 0))} (fixture under `fixtures/minimal-ts/`)."
    )
    lines.append("")

    lines.append("### Rust — quick signals")
    lines.append("")
    pe = rs.get("parse_errors")
    n_pe = len(pe) if isinstance(pe, list) else 0
    cyc = rs.get("cycles_raw")
    n_cyc = len(cyc) if isinstance(cyc, list) else 0
    cc = rs.get("complexity") or []
    top = max(cc, key=lambda x: x.get("cc", 0)) if cc else {}
    uw_aud = rs.get("unwrap_audit")
    uw = uw_aud.get("total", 0) if isinstance(uw_aud, dict) else 0
    todo_o = rs.get("todo_audit")
    todo_t = todo_o.get("total", 0) if isinstance(todo_o, dict) else 0
    de = rs.get("dead_exports")
    n_de = len(de) if isinstance(de, list) else 0

    lines.append(
        f"- **Internal import cycles:** **{n_cyc}** — "
        f"{'no circular module chains detected.' if n_cyc == 0 else 'see Rust section for chains.'}"
    )
    lines.append(
        f"- **Parse errors:** **{n_pe}** — "
        f"{'all `.rs` files parsed.' if n_pe == 0 else 'fix syntax before trusting other Rust metrics.'}"
    )
    if top:
        lines.append(
            f"- **Peak cyclomatic complexity:** **CC={top.get('cc')}** — `{top.get('name', '')}` "
            f"at `{top.get('file', '')}:{top.get('line', '')}` (largest hotspot by CC)."
        )
    lines.append(f"- **`.unwrap()` / `.expect()` call sites:** **{uw}** (see unwrap audit for files).")
    lines.append(f"- **TODO / FIXME-style markers:** **{todo_t}** (see audit for locations).")
    lines.append(
        f"- **Heuristic dead exports:** **{n_de}** — triage only; many are normal for internal `pub` APIs."
    )
    lines.append("")


def section_python(lines: list[str], d: dict[str, Any] | None) -> None:
    lines.append("## Python")
    lines.append("")
    lines.append(
        "_Scope: `.py` under the scan root except paths matching `--exclude scripts/`._"
    )
    lines.append("")
    if not d:
        lines.append("_No data._")
        lines.append("")
        return

    s = d.get("summary") or {}
    h3(lines, "Summary")
    for k, lab in [
        ("files", "Files"),
        ("lines", "Lines"),
        ("functions", "Functions"),
        ("classes", "Classes"),
    ]:
        if k in s:
            lines.append(f"- **{lab}:** {s[k]}")
    lines.append(f"- **Package (import root):** `{d.get('package', '')}`")
    lines.append("")

    inv = d.get("inventory") or {}
    h3(lines, "Inventory")
    for key, title, cap in [
        ("files_by_lines", "Files by line count", TOP_FILES),
        ("largest_functions", "Largest functions", TOP_FUNCS),
        ("largest_classes", "Largest classes", TOP_TYPES),
    ]:
        rows = inv.get(key) or []
        if not rows:
            lines.append(f"- **{title}:** _(none)_")
            continue
        lines.append(f"- **{title}** (up to {cap}):")
        for row in rows[:cap]:
            if isinstance(row, dict):
                if "name" in row and "file" in row:
                    lines.append(
                        f"  - {row.get('lines', '?')} lines — `{row.get('name')}` "
                        f"(`{row.get('file')}:{row.get('line', '')}`)"
                    )
                elif "lines" in row and "file" in row:
                    lines.append(f"  - {row.get('lines')} lines — `{row.get('file')}`")
                else:
                    lines.append(f"  - {row}")
            else:
                lines.append(f"  - {row}")
    lines.append("")

    md_complexity(lines, d.get("complexity"), "python")
    md_nesting(lines, d.get("nesting"))
    md_imports(lines, d.get("imports"), "Python")
    md_coupling(lines, d.get("coupling"))
    md_cycles(lines, d.get("cycles_raw"))
    md_dead_exports(lines, d.get("dead_exports"))
    md_todo_audit(lines, d.get("todo_audit"))

    dec = d.get("decorators")
    if isinstance(dec, list) and dec:
        h3(lines, "Decorators")
        for row in dec[:DECORATORS_MAX]:
            lines.append(f"- {row}")
        if len(dec) > DECORATORS_MAX:
            lines.append(f"- … and {len(dec) - DECORATORS_MAX} more")
        lines.append("")

    routes = d.get("routes")
    if isinstance(routes, list) and routes:
        h3(lines, "Routes (FastAPI-style)")
        for row in routes[:ROUTES_MAX]:
            lines.append(f"- {row}")
        if len(routes) > ROUTES_MAX:
            lines.append(f"- … and {len(routes) - ROUTES_MAX} more")
        lines.append("")

    silent = d.get("silent_excepts")
    if isinstance(silent, list) and silent:
        h3(lines, "Silent exception handlers")
        for row in silent[:SILENT_MAX]:
            lines.append(f"- {row}")
        if len(silent) > SILENT_MAX:
            lines.append(f"- … and {len(silent) - SILENT_MAX} more")
        lines.append("")

    md_parse_errors(lines, d.get("parse_errors"))


def section_typescript(lines: list[str], d: dict[str, Any] | None) -> None:
    lines.append("## TypeScript and JavaScript")
    lines.append("")
    lines.append("_Scope: `.ts`/`.tsx`/`.js`/`.jsx` under the scan root._")
    lines.append("")
    if not d:
        lines.append("_No data._")
        lines.append("")
        return

    s = d.get("summary") or {}
    h3(lines, "Summary")
    for k, lab in [
        ("files", "Files"),
        ("lines", "Lines"),
        ("functions", "Functions / consts"),
        ("classes", "Classes"),
        ("components", "React components"),
        ("custom_hooks", "Custom hooks"),
        ("internal_imports", "Internal imports"),
        ("external_imports", "External imports"),
    ]:
        if k in s:
            lines.append(f"- **{lab}:** {s[k]}")
    lines.append(f"- **Alias prefix:** `{d.get('alias_prefix', '')}`")
    lines.append("")

    inv = d.get("inventory") or {}
    h3(lines, "Inventory")
    for key, title, cap in [
        ("files_by_lines", "Files by line count", TOP_FILES),
        ("largest_functions", "Largest functions", TOP_FUNCS),
        ("largest_classes", "Largest classes", TOP_TYPES),
    ]:
        rows = inv.get(key) or []
        if not rows:
            lines.append(f"- **{title}:** _(none)_")
            continue
        lines.append(f"- **{title}** (up to {cap}):")
        for row in rows[:cap]:
            if isinstance(row, dict):
                if "name" in row and "file" in row:
                    lines.append(
                        f"  - {row.get('lines', '?')} lines — `{row.get('name')}` "
                        f"(`{row.get('file')}:{row.get('line', '')}`)"
                    )
                elif "lines" in row and "file" in row:
                    lines.append(f"  - {row.get('lines')} lines — `{row.get('file')}`")
                else:
                    lines.append(f"  - {row}")
            else:
                lines.append(f"  - {row}")
    lines.append("")

    md_complexity(lines, d.get("complexity"), "ts")
    md_nesting(lines, d.get("nesting"))
    md_imports(lines, d.get("imports"), "TypeScript")
    md_coupling(lines, d.get("coupling"))
    md_cycles(lines, d.get("cycles_raw"))
    md_dead_exports(lines, d.get("dead_exports"))
    md_todo_audit(lines, d.get("todo_audit"))

    any_a = d.get("any_audit")
    if isinstance(any_a, dict) and (any_a.get("total") or 0) > 0:
        h3(lines, "`any` audit")
        lines.append(f"- **Total:** {any_a.get('total', 0)}")
        for row in (any_a.get("by_file") or [])[:ANY_FILES_MAX]:
            if isinstance(row, dict):
                lines.append(f"  - `{row.get('file', '')}` — {row.get('count', 0)}")
        lines.append("")

    ts_dir = d.get("ts_directives")
    if isinstance(ts_dir, dict) and (ts_dir.get("total") or 0) > 0:
        h3(lines, "TypeScript directives")
        lines.append(f"- **Total:** {ts_dir.get('total', 0)}")
        for row in ts_dir.get("by_directive") or []:
            if isinstance(row, dict):
                lines.append(f"  - **{row.get('directive', '')}:** {row.get('count', 0)}")
        lines.append("")

    eslint = d.get("eslint_disables")
    if isinstance(eslint, dict) and (eslint.get("total") or 0) > 0:
        h3(lines, "ESLint disable comments")
        lines.append(f"- **Total:** {eslint.get('total', 0)} (unique rules: {eslint.get('unique_rules', '—')})")
        for row in (eslint.get("by_rule") or [])[:ESLINT_RULES_MAX]:
            if isinstance(row, dict):
                lines.append(f"  - **{row.get('rule', '')}:** {row.get('count', 0)}")
        lines.append("")

    cons = d.get("console_debugger")
    if isinstance(cons, dict) and (cons.get("total") or 0) > 0:
        h3(lines, "Console / debugger")
        lines.append(f"- **Total:** {cons.get('total', 0)}")
        items = cons.get("items") or []
        for row in items[:CONSOLE_ITEMS_MAX]:
            lines.append(f"  - {row}")
        lines.append("")

    hooks = d.get("hooks")
    if isinstance(hooks, dict):
        freq = hooks.get("frequency") or {}
        if freq:
            h3(lines, "Hooks — frequency")
            for k, v in list(freq.items())[:20]:
                lines.append(f"  - `{k}`: {v}")
            lines.append("")
        chi = hooks.get("custom_hooks_inventory") or []
        if chi:
            h3(lines, "Custom hooks inventory")
            for row in chi[:25]:
                lines.append(f"  - {row}")
            lines.append("")
        hc = hooks.get("heavy_components") or []
        if hc:
            h3(lines, "Heavy components")
            for row in hc[:15]:
                lines.append(f"  - {row}")
            lines.append("")

    mobx = d.get("mobx_observer")
    if isinstance(mobx, list) and mobx:
        h3(lines, "MobX observer gaps")
        for row in mobx[:20]:
            lines.append(f"  - {row}")
        lines.append("")

    props = d.get("component_props")
    if isinstance(props, list) and props:
        h3(lines, "Component props (sample)")
        for row in props[:15]:
            lines.append(f"  - {row}")
        lines.append("")

    ib = d.get("import_boundaries")
    if isinstance(ib, list) and ib:
        h3(lines, "Import boundary violations")
        for row in ib[:25]:
            lines.append(f"  - {row}")
        lines.append("")

    orm = d.get("orm_case_check")
    if isinstance(orm, list) and orm:
        h3(lines, "ORM case check")
        for row in orm[:25]:
            lines.append(f"  - {row}")
        lines.append("")

    silent = d.get("silent_catches")
    if isinstance(silent, list) and silent:
        h3(lines, "Silent catch handlers")
        for row in silent[:SILENT_MAX]:
            lines.append(f"  - {row}")
        lines.append("")


def section_rust(lines: list[str], d: dict[str, Any] | None) -> None:
    lines.append("## Rust")
    lines.append("")
    lines.append("_Scope: all `.rs` files under the scan root (crate / workspace subtree)._")
    lines.append("")
    if not d:
        lines.append("_No data._")
        lines.append("")
        return

    s = d.get("summary") or {}
    h3(lines, "Summary")
    for k, lab in [
        ("files", "Files"),
        ("lines", "Lines"),
        ("functions", "Functions"),
        ("structs_enums", "Structs / enums"),
        ("traits", "Traits"),
        ("internal_imports", "Internal imports"),
        ("external_imports", "External imports"),
        ("unsafe_blocks", "Unsafe blocks (count)"),
        ("unwrap_expect_total", "unwrap/expect sites"),
    ]:
        if k in s:
            lines.append(f"- **{lab}:** {s[k]}")
    if "unwrap_expect_total" not in s and "unwrap_expect_count" in s:
        lines.append(f"- **unwrap/expect sites:** {s['unwrap_expect_count']}")
    lines.append("")

    inv = d.get("inventory") or {}
    h3(lines, "Inventory")
    for key, title, cap in [
        ("files_by_lines", "Files by line count", TOP_FILES),
        ("largest_functions", "Largest functions", TOP_FUNCS),
        ("largest_types", "Largest structs / enums", TOP_TYPES),
    ]:
        rows = inv.get(key) or []
        if not rows:
            lines.append(f"- **{title}:** _(none)_")
            continue
        lines.append(f"- **{title}** (up to {cap}):")
        for row in rows[:cap]:
            if isinstance(row, dict):
                if "lines" in row and "file" in row and "name" not in row:
                    lines.append(f"  - {row.get('lines')} lines — `{row.get('file')}`")
                elif "name" in row:
                    kind = row.get("kind", "")
                    kind_s = f" ({kind})" if kind else ""
                    lines.append(
                        f"  - {row.get('lines', '?')} lines{kind_s} — `{row.get('name')}` (`{row.get('file')}:{row.get('line', '')}`)"
                    )
                else:
                    lines.append(f"  - {row}")
            else:
                lines.append(f"  - {row}")
    lines.append("")

    md_complexity(lines, d.get("complexity"), "rust")
    md_nesting(lines, d.get("nesting"))
    md_imports(
        lines,
        d.get("imports"),
        "Rust",
        footnote="Internal targets are module ids from the Rust scanner (path-like strings; counts are internal `use` references).",
    )
    md_coupling(lines, d.get("coupling"))
    md_cycles(lines, d.get("cycles_raw"))
    md_dead_exports(lines, d.get("dead_exports"))

    ua = d.get("unsafe_audit")
    if isinstance(ua, dict):
        h3(lines, "Unsafe audit")
        lines.append(f"- **Unsafe functions:** {ua.get('unsafe_functions', 0)}")
        lines.append(f"- **Unsafe blocks:** {ua.get('unsafe_blocks', 0)}")
        bf = ua.get("by_file") or []
        if bf:
            lines.append("- **By file:**")
            for row in bf[:20]:
                if isinstance(row, dict):
                    lines.append(
                        f"  - `{row.get('file', '')}` — blocks: {row.get('unsafe_blocks', row.get('blocks', 0))}, "
                        f"unsafe fns: {row.get('unsafe_functions', row.get('unsafe_fns', 0))}"
                    )
                else:
                    lines.append(f"  - {row}")
        lines.append("")

    uw = d.get("unwrap_audit")
    if isinstance(uw, dict):
        h3(lines, "unwrap / expect audit")
        lines.append(f"- **Total call sites:** {uw.get('total', 0)}")
        bf = uw.get("by_file") or []
        if bf:
            lines.append(f"- **By file (up to {UNWRAP_FILES_MAX}):**")
            for row in bf[:UNWRAP_FILES_MAX]:
                if isinstance(row, dict):
                    lines.append(f"  - `{row.get('file', '')}` — {row.get('count', 0)}")
                else:
                    lines.append(f"  - {row}")
        lines.append("")

    al = d.get("allow_lints")
    if isinstance(al, dict):
        h3(lines, "#[allow(...)] audit")
        lines.append(f"- **Total allow attributes (expanded):** {al.get('total', 0)}")
        for row in (al.get("by_rule") or [])[:ALLOW_RULES_MAX]:
            if isinstance(row, dict):
                lines.append(f"  - **{row.get('rule', '')}:** {row.get('count', 0)}")
        lines.append("")

    dr = d.get("derive_audit")
    if isinstance(dr, dict):
        h3(lines, "Derive macro audit")
        lines.append(f"- **Total derive uses:** {dr.get('total', 0)}")
        for row in (dr.get("by_derive") or [])[:DERIVE_MAX]:
            if isinstance(row, dict):
                lines.append(f"  - **{row.get('derive', '')}:** {row.get('count', 0)}")
        lines.append("")

    md_todo_audit(lines, d.get("todo_audit"))

    traits = d.get("traits_inventory")
    if isinstance(traits, list) and traits:
        h3(lines, "Trait inventory")
        for row in traits[:TRAITS_MAX]:
            if isinstance(row, dict):
                lines.append(
                    f"- `{row.get('name', '')}` (`{row.get('file', '')}:{row.get('line', '')}`) — `{row.get('visibility', '')}`"
                )
            else:
                lines.append(f"- {row}")
        if len(traits) > TRAITS_MAX:
            lines.append(f"- … and {len(traits) - TRAITS_MAX} more")
        lines.append("")

    md_parse_errors(lines, d.get("parse_errors"))


def main() -> int:
    root = Path(__file__).resolve().parents[1]
    rust_bin = root / "rust" / "target" / "release" / "ast-scan"
    if not rust_bin.is_file():
        subprocess.run(
            ["cargo", "build", "--release"],
            cwd=root / "rust",
            check=True,
        )
    jpath = root / ".scan-report.json"
    # Full JSON including parse-errors sections (no --skip).
    # Exclude `scripts/` so this generator’s `.py` file does not inflate Python metrics.
    subprocess.run(
        [
            str(rust_bin),
            str(root),
            "--json",
            "--exclude",
            "scripts/",
        ],
        stdout=open(jpath, "w"),
        check=True,
    )
    data = json.loads(jpath.read_text())

    lines: list[str] = []
    lines.append("# AST-SCAN repository digest")
    lines.append("")
    lines.append(
        "_Automated snapshot: the **ast-scan** CLI is run on this repo and summarized below._"
    )
    lines.append("")
    lines.append(
        f"**Generated:** {datetime.now(timezone.utc).strftime('%Y-%m-%d %H:%M')} UTC"
    )
    lines.append("")
    lines.append("**Command**")
    lines.append("")
    lines.append("```text")
    lines.append("ast-scan <repo-root> --json --exclude scripts/")
    lines.append("```")
    lines.append("")
    lines.append(
        "Multi-language JSON (Python, TypeScript, Rust). The `scripts/` exclude applies only to which `.py` "
        "files are scanned. Parse-error sections are included when non-empty."
    )
    lines.append("")
    lines.append(f"**`report_title` in JSON:** `{data.get('report_title', '')}`")
    lines.append("")

    table_of_contents(lines)
    divider(lines)
    executive_summary(lines, data)
    divider(lines)
    how_to_read(lines)
    divider(lines)

    section_python(lines, data.get("python"))
    divider(lines)
    section_typescript(lines, data.get("typescript"))
    divider(lines)
    section_rust(lines, data.get("rust"))
    divider(lines)

    lines.append("## Regenerate")
    lines.append("")
    lines.append("```bash")
    lines.append("python3 scripts/self_scan_insights.py")
    lines.append("```")
    lines.append("")
    lines.append("---")
    lines.append("")
    lines.append(
        "_Full machine-readable output: `.scan-report.json` (gitignored). "
        "This digest is meant for humans; use the JSON for tooling and diffs._"
    )

    out = root / "SCAN_INSIGHTS.md"
    out.write_text("\n".join(lines))
    print(f"Wrote {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
