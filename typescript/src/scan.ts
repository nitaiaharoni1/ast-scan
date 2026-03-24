#!/usr/bin/env node
/**
 * AST-based codebase scan for TypeScript/JavaScript (complexity, imports, React hooks).
 */

import * as ts from "typescript";
import * as fs from "node:fs";
import * as path from "node:path";
import { fileURLToPath } from "node:url";

// ── Types ────────────────────────────────────────────────────────────────────

interface FuncInfo {
  name: string;
  file: string;
  line: number;
  endLine: number;
  lineCount: number;
  complexity: number;
  exported: boolean;
  isComponent: boolean;
  props: string[];
  hooks: string[];
}

interface ImportInfo {
  source: string;
  specifiers: string[];
  isInternal: boolean;
  resolvedPath: string;
}

interface FileData {
  relPath: string;
  absPath: string;
  lineCount: number;
  functions: FuncInfo[];
  imports: ImportInfo[];
  exports: string[];
}

interface CliConfig {
  scanRoot: string;
  aliasPrefix: string;
  top: number;
  json: boolean;
  skip: Set<string>;
  title: string;
}

// ── File collection ──────────────────────────────────────────────────────────

function collectFiles(dir: string, exts: string[]): string[] {
  const results: string[] = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      results.push(...collectFiles(full, exts));
    } else if (exts.some((e) => entry.name.endsWith(e))) {
      results.push(full);
    }
  }
  return results.sort();
}

function relPath(absPath: string, scanRoot: string): string {
  return path.relative(path.resolve(scanRoot, ".."), absPath);
}

function normalizeAliasPrefix(alias: string): string {
  const t = alias.trim();
  if (!t.endsWith("/")) {
    return `${t}/`;
  }
  return t;
}

// ── Import resolution ────────────────────────────────────────────────────────

function resolveImport(
  importPath: string,
  fromFile: string,
  scanRoot: string,
  aliasPrefix: string,
): { isInternal: boolean; resolved: string } {
  if (importPath.startsWith(aliasPrefix)) {
    const mapped = importPath.slice(aliasPrefix.length);
    return { isInternal: true, resolved: mapped };
  }
  if (importPath.startsWith(".")) {
    const dir = path.dirname(path.relative(scanRoot, fromFile));
    const resolved = path.normalize(path.join(dir, importPath));
    return { isInternal: true, resolved };
  }
  return { isInternal: false, resolved: importPath };
}

function normalizeModulePath(p: string): string {
  let cleaned = p.replace(/\\/g, "/");
  for (const ext of [".tsx", ".ts", ".jsx", ".js"]) {
    if (cleaned.endsWith(ext)) {
      cleaned = cleaned.slice(0, -ext.length);
    }
  }
  if (cleaned.endsWith("/index")) {
    cleaned = cleaned.slice(0, -"/index".length);
  }
  return cleaned;
}

// ── Complexity ───────────────────────────────────────────────────────────────

function computeComplexity(node: ts.Node): number {
  let cc = 1;
  function walk(n: ts.Node) {
    switch (n.kind) {
      case ts.SyntaxKind.IfStatement:
      case ts.SyntaxKind.ForStatement:
      case ts.SyntaxKind.ForInStatement:
      case ts.SyntaxKind.ForOfStatement:
      case ts.SyntaxKind.WhileStatement:
      case ts.SyntaxKind.DoStatement:
      case ts.SyntaxKind.ConditionalExpression:
      case ts.SyntaxKind.CatchClause:
        cc++;
        break;
      case ts.SyntaxKind.SwitchStatement: {
        const sw = n as ts.SwitchStatement;
        cc += sw.caseBlock.clauses.filter(
          (c) => c.kind === ts.SyntaxKind.CaseClause,
        ).length;
        break;
      }
      case ts.SyntaxKind.BinaryExpression: {
        const bin = n as ts.BinaryExpression;
        if (
          bin.operatorToken.kind === ts.SyntaxKind.AmpersandAmpersandToken ||
          bin.operatorToken.kind === ts.SyntaxKind.BarBarToken ||
          bin.operatorToken.kind === ts.SyntaxKind.QuestionQuestionToken
        ) {
          cc++;
        }
        break;
      }
    }
    ts.forEachChild(n, walk);
  }
  ts.forEachChild(node, walk);
  return cc;
}

function returnsJsx(node: ts.Node): boolean {
  let found = false;
  function walk(n: ts.Node) {
    if (found) return;
    if (
      n.kind === ts.SyntaxKind.JsxElement ||
      n.kind === ts.SyntaxKind.JsxSelfClosingElement ||
      n.kind === ts.SyntaxKind.JsxFragment
    ) {
      found = true;
      return;
    }
    ts.forEachChild(n, walk);
  }
  ts.forEachChild(node, walk);
  return found;
}

function extractHooks(node: ts.Node): string[] {
  const hooks: string[] = [];
  function walk(n: ts.Node) {
    if (ts.isCallExpression(n)) {
      let name: string | undefined;
      if (ts.isIdentifier(n.expression)) {
        name = n.expression.text;
      } else if (ts.isPropertyAccessExpression(n.expression)) {
        name = n.expression.name.text;
      }
      if (name && /^use[A-Z]/.test(name) && !hooks.includes(name)) {
        hooks.push(name);
      }
    }
    ts.forEachChild(n, walk);
  }
  ts.forEachChild(node, walk);
  return hooks;
}

function extractProps(
  node: ts.FunctionDeclaration | ts.ArrowFunction | ts.FunctionExpression,
  _sf: ts.SourceFile,
): string[] {
  const props: string[] = [];
  const params = node.parameters;
  if (params.length === 0) return props;

  const firstParam = params[0];
  if (ts.isObjectBindingPattern(firstParam.name)) {
    for (const elem of firstParam.name.elements) {
      if (ts.isBindingElement(elem) && ts.isIdentifier(elem.name)) {
        props.push(elem.name.text);
      }
    }
  } else if (firstParam.type && ts.isTypeReferenceNode(firstParam.type)) {
    if (ts.isIdentifier(firstParam.type.typeName)) {
      props.push(`[type: ${firstParam.type.typeName.text}]`);
    }
  }
  return props;
}

function analyzeFile(
  absPath: string,
  scanRoot: string,
  aliasPrefix: string,
): FileData {
  const source = fs.readFileSync(absPath, "utf-8");
  const sf = ts.createSourceFile(
    absPath,
    source,
    ts.ScriptTarget.Latest,
    true,
    absPath.endsWith(".tsx") ? ts.ScriptKind.TSX : ts.ScriptKind.TS,
  );

  const lines = source.split("\n").length;
  const functions: FuncInfo[] = [];
  const imports: ImportInfo[] = [];
  const exports: string[] = [];

  function isExported(node: ts.Node): boolean {
    if (!ts.canHaveModifiers(node)) return false;
    const mods = ts.getModifiers(node);
    return mods?.some((m) => m.kind === ts.SyntaxKind.ExportKeyword) ?? false;
  }

  function getLineNum(pos: number): number {
    return sf.getLineAndCharacterOfPosition(pos).line + 1;
  }

  function processFunctionLike(
    name: string,
    node:
      | ts.FunctionDeclaration
      | ts.ArrowFunction
      | ts.FunctionExpression
      | ts.MethodDeclaration,
    exported: boolean,
  ) {
    const startLine = getLineNum(node.getStart(sf));
    const endLine = getLineNum(node.getEnd());
    const isComp =
      /^[A-Z]/.test(name) &&
      !name.endsWith("Provider") &&
      returnsJsx(node);
    const hooks = isComp ? extractHooks(node) : [];
    const props =
      isComp &&
      (ts.isFunctionDeclaration(node) ||
        ts.isArrowFunction(node) ||
        ts.isFunctionExpression(node))
        ? extractProps(node, sf)
        : [];

    functions.push({
      name,
      file: relPath(absPath, scanRoot),
      line: startLine,
      endLine,
      lineCount: endLine - startLine + 1,
      complexity: computeComplexity(node),
      exported,
      isComponent: isComp,
      props,
      hooks,
    });
    if (exported) exports.push(name);
  }

  for (const stmt of sf.statements) {
    if (ts.isImportDeclaration(stmt) && ts.isStringLiteral(stmt.moduleSpecifier)) {
      const impPath = stmt.moduleSpecifier.text;
      const { isInternal, resolved } = resolveImport(impPath, absPath, scanRoot, aliasPrefix);
      const specifiers: string[] = [];
      if (stmt.importClause) {
        if (stmt.importClause.name) {
          specifiers.push(stmt.importClause.name.text);
        }
        const bindings = stmt.importClause.namedBindings;
        if (bindings) {
          if (ts.isNamedImports(bindings)) {
            for (const spec of bindings.elements) {
              specifiers.push(spec.name.text);
            }
          } else if (ts.isNamespaceImport(bindings)) {
            specifiers.push(`* as ${bindings.name.text}`);
          }
        }
      }
      imports.push({ source: impPath, specifiers, isInternal, resolvedPath: resolved });
    }

    if (ts.isFunctionDeclaration(stmt) && stmt.name) {
      processFunctionLike(stmt.name.text, stmt, isExported(stmt));
    }

    if (ts.isVariableStatement(stmt)) {
      const exp = isExported(stmt);
      for (const decl of stmt.declarationList.declarations) {
        if (
          ts.isIdentifier(decl.name) &&
          decl.initializer &&
          (ts.isArrowFunction(decl.initializer) ||
            ts.isFunctionExpression(decl.initializer))
        ) {
          processFunctionLike(decl.name.text, decl.initializer, exp);
        } else if (ts.isIdentifier(decl.name) && exp) {
          exports.push(decl.name.text);
        }
      }
    }

    if (ts.isExportDeclaration(stmt)) {
      if (stmt.exportClause && ts.isNamedExports(stmt.exportClause)) {
        for (const spec of stmt.exportClause.elements) {
          exports.push(spec.name.text);
        }
      }
    }

    if (ts.isClassDeclaration(stmt) && stmt.name) {
      const exp = isExported(stmt);
      if (exp) exports.push(stmt.name.text);
    }

    if (
      (ts.isTypeAliasDeclaration(stmt) || ts.isInterfaceDeclaration(stmt)) &&
      isExported(stmt)
    ) {
      exports.push(stmt.name.text);
    }

    if (ts.isEnumDeclaration(stmt) && isExported(stmt)) {
      exports.push(stmt.name.text);
    }
  }

  return { relPath: relPath(absPath, scanRoot), absPath, lineCount: lines, functions, imports, exports };
}

function findCycles(graph: Map<string, Set<string>>): string[][] {
  const WHITE = 0,
    GRAY = 1,
    BLACK = 2;
  const color = new Map<string, number>();
  const pathStack: string[] = [];
  const cycles: string[][] = [];

  function dfs(u: string) {
    color.set(u, GRAY);
    pathStack.push(u);
    const neighbors = [...(graph.get(u) ?? [])].sort();
    for (const v of neighbors) {
      const vc = color.get(v) ?? WHITE;
      if (vc === GRAY) {
        const idx = pathStack.indexOf(v);
        cycles.push([...pathStack.slice(idx), v]);
      } else if (vc === WHITE) {
        dfs(v);
      }
    }
    pathStack.pop();
    color.set(u, BLACK);
  }

  for (const node of [...graph.keys()].sort()) {
    if ((color.get(node) ?? WHITE) === WHITE) {
      dfs(node);
    }
  }
  return cycles;
}

function uniqueCycles(cycles: string[][]): string[][] {
  const seen = new Set<string>();
  const unique: string[][] = [];
  for (const c of cycles) {
    const key = [...c.slice(0, -1)].sort().join("|");
    if (!seen.has(key)) {
      seen.add(key);
      unique.push(c);
    }
  }
  return unique;
}

// ── Analysis ─────────────────────────────────────────────────────────────────

export function analyzeTypeScript(scanRoot: string, aliasPrefix: string): Record<string, unknown> {
  const root = path.resolve(scanRoot);
  if (!fs.existsSync(root) || !fs.statSync(root).isDirectory()) {
    throw new Error(`Not a directory: ${root}`);
  }

  const files = collectFiles(root, [".ts", ".tsx"]);
  const allData: FileData[] = [];

  for (const f of files) {
    try {
      allData.push(analyzeFile(f, root, aliasPrefix));
    } catch (err) {
      console.error(`  SKIP: ${relPath(f, root)}: ${err}`);
    }
  }

  const allFunctions = allData.flatMap((d) => d.functions);
  const allImports = allData.flatMap((d) =>
    d.imports.map((i) => ({ ...i, fromFile: d.relPath })),
  );
  const components = allFunctions.filter((f) => f.isComponent);
  const customHookFns = allFunctions.filter(
    (f) => /^use[A-Z]/.test(f.name) && f.exported,
  );

  const graph = new Map<string, Set<string>>();
  const inDegree = new Map<string, number>();
  const allModules = new Set<string>();
  const importedNamesMap = new Map<string, Set<string>>();
  const entryPoints = new Set(["App", "main"]);

  for (const d of allData) {
    const fromMod = normalizeModulePath(path.relative(root, d.absPath));
    allModules.add(fromMod);
  }

  for (const d of allData) {
    const fromMod = normalizeModulePath(path.relative(root, d.absPath));
    for (const imp of d.imports) {
      if (!imp.isInternal) continue;
      const toMod = normalizeModulePath(imp.resolvedPath);
      if (!graph.has(fromMod)) graph.set(fromMod, new Set());
      graph.get(fromMod)!.add(toMod);
      inDegree.set(toMod, (inDegree.get(toMod) ?? 0) + 1);

      if (!importedNamesMap.has(toMod)) importedNamesMap.set(toMod, new Set());
      for (const s of imp.specifiers) {
        importedNamesMap.get(toMod)!.add(s);
      }
    }
  }

  const deadExports: { module: string; name: string }[] = [];
  for (const d of allData) {
    const mod = normalizeModulePath(path.relative(root, d.absPath));
    const baseName = path.basename(d.relPath).replace(/\.(tsx?|jsx?)$/, "");
    if (entryPoints.has(baseName)) continue;
    if (d.relPath.endsWith("/index.ts") || d.relPath.endsWith("/index.tsx")) continue;

    const imported = importedNamesMap.get(mod) ?? new Set();
    for (const exp of d.exports) {
      if (!imported.has(exp) && !exp.startsWith("_")) {
        deadExports.push({ module: mod, name: exp });
      }
    }
  }
  deadExports.sort((a, b) => a.module.localeCompare(b.module));

  const hookFreq = new Map<string, number>();
  for (const comp of components) {
    for (const h of comp.hooks) {
      hookFreq.set(h, (hookFreq.get(h) ?? 0) + 1);
    }
  }

  const totalLines = allData.reduce((s, d) => s + d.lineCount, 0);
  const internalImports = allImports.filter((i) => i.isInternal);
  const externalImports = allImports.filter((i) => !i.isInternal);

  const extFreq = new Map<string, number>();
  for (const imp of externalImports) {
    const pkg = imp.source.startsWith("@")
      ? imp.source.split("/").slice(0, 2).join("/")
      : imp.source.split("/")[0];
    extFreq.set(pkg, (extFreq.get(pkg) ?? 0) + 1);
  }

  const totalEdges = [...graph.values()].reduce((s, v) => s + v.size, 0);
  const rawCycles = findCycles(graph);
  const cyclesUnique = uniqueCycles(rawCycles);

  const complexityRows = [...allFunctions]
    .sort((a, b) => b.complexity - a.complexity)
    .map((fn) => ({
      name: fn.name,
      cc: fn.complexity,
      file: fn.file,
      line: fn.line,
      is_component: fn.isComponent,
    }));

  return {
    scanner: "typescript",
    scan_root: root,
    alias_prefix: aliasPrefix,
    summary: {
      files: allData.length,
      lines: totalLines,
      functions: allFunctions.length,
      components: components.length,
      custom_hooks: customHookFns.length,
      internal_imports: internalImports.length,
      external_imports: externalImports.length,
    },
    inventory: {
      files_by_lines: [...allData]
        .sort((a, b) => b.lineCount - a.lineCount)
        .map((d) => ({ file: d.relPath, lines: d.lineCount })),
      largest_functions: [...allFunctions]
        .sort((a, b) => b.lineCount - a.lineCount)
        .map((fn) => ({
          name: fn.name,
          lines: fn.lineCount,
          file: fn.file,
          line: fn.line,
          is_component: fn.isComponent,
        })),
    },
    complexity: complexityRows,
    imports: {
      modules: allModules.size,
      edges: totalEdges,
      top_imported: [...inDegree.entries()]
        .sort((a, b) => b[1] - a[1])
        .map(([mod, count]) => ({ module: mod, count })),
      external_packages: [...extFreq.entries()]
        .sort((a, b) => b[1] - a[1])
        .map(([pkg, count]) => ({ package: pkg, count })),
    },
    cycles: cyclesUnique.map((c) => c.join(" -> ")),
    cycles_raw: cyclesUnique,
    dead_exports: deadExports,
    component_props: components
      .filter((c) => c.props.length > 0)
      .sort((a, b) => a.name.localeCompare(b.name))
      .map((c) => ({
        name: c.name,
        file: c.file,
        line: c.line,
        props: c.props,
      })),
    hooks: {
      frequency: [...hookFreq.entries()]
        .sort((a, b) => b[1] - a[1])
        .map(([hook, count]) => ({
          hook,
          count,
          is_custom: customHookFns.some((f) => f.name === hook),
        })),
      custom_hooks_inventory: [...customHookFns]
        .sort((a, b) => a.name.localeCompare(b.name))
        .map((fn) => ({
          name: fn.name,
          lines: fn.lineCount,
          file: fn.file,
          line: fn.line,
        })),
      heavy_components: components
        .filter((c) => c.hooks.length >= 3)
        .sort((a, b) => b.hooks.length - a.hooks.length)
        .map((c) => ({
          name: c.name,
          file: c.file,
          line: c.line,
          hook_count: c.hooks.length,
          hooks: c.hooks,
        })),
    },
  };
}

function printTextReport(data: Record<string, unknown>, cfg: CliConfig): void {
  const skip = cfg.skip;
  const top = cfg.top;
  const ccTop = Math.max(top, 30);
  const sep = "=".repeat(72);

  const s = data.summary as Record<string, number>;

  console.log(sep);
  console.log(`  ${cfg.title}`);
  console.log(sep);
  console.log();
  console.log(`  Files analyzed:    ${s.files}`);
  console.log(`  Total lines:       ${s.lines.toLocaleString()}`);
  console.log(`  Functions/consts:  ${s.functions}`);
  console.log(`  React components:  ${s.components}`);
  console.log(`  Custom hooks:      ${s.custom_hooks}`);
  console.log(`  Internal imports:  ${s.internal_imports}`);
  console.log(`  External imports:  ${s.external_imports}`);
  console.log();

  if (!skip.has("inventory")) {
    const inv = data.inventory as {
      files_by_lines: { file: string; lines: number }[];
      largest_functions: {
        name: string;
        lines: number;
        file: string;
        line: number;
        is_component: boolean;
      }[];
    };
    console.log(sep);
    console.log(`  1. INVENTORY — Top ${top} Largest Files`);
    console.log(sep);
    for (const row of inv.files_by_lines.slice(0, top)) {
      console.log(`  ${String(row.lines).padStart(5)} lines  ${row.file}`);
    }
    console.log();
    console.log(`  Top ${top} Largest Functions`);
    console.log("  " + "-".repeat(60));
    for (const fn of inv.largest_functions.slice(0, top)) {
      const tag = fn.is_component ? " [component]" : "";
      console.log(
        `  ${String(fn.lines).padStart(5)} lines  ${fn.name}${tag}  [${fn.file}:${fn.line}]`,
      );
    }
    console.log();
  }

  if (!skip.has("complexity")) {
    const complexity = data.complexity as {
      name: string;
      cc: number;
      file: string;
      line: number;
      is_component: boolean;
    }[];
    console.log(sep);
    console.log(`  2. CYCLOMATIC COMPLEXITY — Top ${ccTop}`);
    console.log(sep);
    for (const fn of complexity.slice(0, ccTop)) {
      const tag = fn.is_component ? " [component]" : "";
      console.log(
        `  CC=${String(fn.cc).padStart(3)}  ${fn.name}${tag}  [${fn.file}:${fn.line}]`,
      );
    }
    console.log();
  }

  if (!skip.has("imports")) {
    const imp = data.imports as {
      modules: number;
      edges: number;
      top_imported: { module: string; count: number }[];
      external_packages: { package: string; count: number }[];
    };
    console.log(sep);
    console.log("  3. IMPORT DEPENDENCY GRAPH");
    console.log(sep);
    console.log(`  Internal modules:      ${imp.modules}`);
    console.log(`  Internal import edges: ${imp.edges}`);
    console.log(`  External packages:     ${imp.external_packages.length}`);
    console.log();
    console.log(`  Top ${top} Most-Imported Internal Modules`);
    console.log("  " + "-".repeat(60));
    for (const row of imp.top_imported.slice(0, top)) {
      console.log(`  ${String(row.count).padStart(4)}x  ${row.module}`);
    }
    console.log();
    console.log(`  Top ${Math.min(15, top)} External Packages`);
    console.log("  " + "-".repeat(60));
    for (const row of imp.external_packages.slice(0, Math.min(15, top))) {
      console.log(`  ${String(row.count).padStart(4)}x  ${row.package}`);
    }
    console.log();
  }

  if (!skip.has("cycles")) {
    console.log(sep);
    console.log("  4. CIRCULAR IMPORTS");
    console.log(sep);
    const raw = data.cycles_raw as string[][];
    if (raw.length > 0) {
      console.log(`  Found ${raw.length} cycle(s):`);
      for (let i = 0; i < raw.length; i++) {
        console.log(`  [${i + 1}] ${raw[i].join(" -> ")}`);
      }
    } else {
      console.log("  None found.");
    }
    console.log();
  }

  if (!skip.has("dead-exports")) {
    console.log(sep);
    console.log("  5. DEAD EXPORTS (exported but never imported internally)");
    console.log(sep);
    const dead = data.dead_exports as { module: string; name: string }[];
    if (dead.length > 0) {
      const cap = Math.max(80, top * 4);
      console.log(`  Found ${dead.length} potentially dead export(s):`);
      for (const de of dead.slice(0, cap)) {
        console.log(`    ${de.module} :: ${de.name}`);
      }
      if (dead.length > cap) {
        console.log(`    ... and ${dead.length - cap} more`);
      }
    } else {
      console.log("  None found.");
    }
    console.log();
  }

  if (!skip.has("component-props")) {
    const props = data.component_props as {
      name: string;
      file: string;
      line: number;
      props: string[];
    }[];
    const s2 = data.summary as { components: number };
    console.log(sep);
    console.log("  6. COMPONENT PROPS");
    console.log(sep);
    console.log(
      `  ${s2.components} components total, ${props.length} with extractable props`,
    );
    console.log();
    for (const comp of props) {
      console.log(`  ${comp.name}  [${comp.file}:${comp.line}]`);
      console.log(`    props: ${comp.props.join(", ")}`);
    }
    console.log();
  }

  if (!skip.has("hooks")) {
    const hooks = data.hooks as {
      frequency: { hook: string; count: number; is_custom: boolean }[];
      custom_hooks_inventory: { name: string; lines: number; file: string; line: number }[];
      heavy_components: {
        name: string;
        file: string;
        line: number;
        hook_count: number;
        hooks: string[];
      }[];
    };
    console.log(sep);
    console.log("  7. HOOK USAGE PATTERNS");
    console.log(sep);
    console.log();
    console.log("  Hook Frequency (across all components)");
    console.log("  " + "-".repeat(60));
    for (const row of hooks.frequency.slice(0, top)) {
      const tag = row.is_custom ? " [custom]" : "";
      console.log(`  ${String(row.count).padStart(4)}x  ${row.hook}${tag}`);
    }
    console.log();
    console.log("  Custom Hooks Inventory");
    console.log("  " + "-".repeat(60));
    for (const fn of hooks.custom_hooks_inventory) {
      console.log(`  ${fn.name}  (${fn.lines} lines)  [${fn.file}:${fn.line}]`);
    }
    console.log();
    console.log("  Per-Component Hook Usage (components using 3+ hooks)");
    console.log("  " + "-".repeat(60));
    for (const comp of hooks.heavy_components.slice(0, top)) {
      console.log(`  ${comp.name}  [${comp.file}:${comp.line}]`);
      console.log(`    hooks (${comp.hook_count}): ${comp.hooks.join(", ")}`);
    }
    console.log();
  }

  console.log(sep);
  console.log("  END OF REPORT");
  console.log(sep);
}

function parseArgv(argv: string[]): CliConfig {
  let scanRoot: string | undefined;
  let alias = "@/";
  let top = 20;
  let json = false;
  const skip: string[] = [];
  let title: string | undefined;

  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--json") {
      json = true;
      continue;
    }
    if (a === "--alias") {
      alias = argv[++i] ?? alias;
      continue;
    }
    if (a === "--top") {
      top = parseInt(argv[++i] ?? "20", 10) || 20;
      continue;
    }
    if (a === "--title") {
      title = argv[++i];
      continue;
    }
    if (a === "--skip") {
      const s = argv[++i];
      if (s) skip.push(s);
      continue;
    }
    if (a === "-h" || a === "--help") {
      printHelp();
      process.exit(0);
    }
    if (!a.startsWith("-")) {
      scanRoot = a;
      continue;
    }
    console.error(`Unknown option: ${a}`);
    printHelp();
    process.exit(2);
  }

  if (!scanRoot) {
    console.error("ast-scan: missing PATH (directory to scan)");
    printHelp();
    process.exit(2);
  }

  const aliasPrefix = normalizeAliasPrefix(alias);
  const resolvedRoot = path.resolve(scanRoot);
  const baseName = path.basename(resolvedRoot);
  const defaultName =
    baseName === "src" || baseName === "lib"
      ? path.basename(path.dirname(resolvedRoot))
      : baseName;
  const baseTitle =
    title ?? `${defaultName.toUpperCase()} — AST ANALYSIS (TypeScript)`;

  return {
    scanRoot,
    aliasPrefix,
    top,
    json,
    skip: new Set(skip),
    title: baseTitle,
  };
}

function printHelp(): void {
  console.error(`Usage: ast-scan <path> [options]

Arguments:
  path              Source directory to scan (e.g. src/)

Options:
  --alias PREFIX    Path alias for internal imports (default: @/)
  --top N           Number of items in ranked sections (default: 20)
  --title TEXT      Report title
  --json            Emit JSON instead of text report
  --skip SECTION    Repeatable. Sections: inventory, complexity, imports,
                    cycles, dead-exports, component-props, hooks
  -h, --help        Show help
`);
}

export function main(argv: string[] = process.argv.slice(2)): void {
  const cfg = parseArgv(argv);
  const data = analyzeTypeScript(cfg.scanRoot, cfg.aliasPrefix);

  if (cfg.json) {
    const out = { ...data, report_title: cfg.title };
    console.log(JSON.stringify(out, null, 2));
    return;
  }

  printTextReport(data, cfg);
}

const entryScript = process.argv[1] && path.resolve(process.argv[1]);
if (entryScript && fileURLToPath(import.meta.url) === entryScript) {
  main();
}
