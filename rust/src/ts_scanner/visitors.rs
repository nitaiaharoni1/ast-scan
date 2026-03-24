//! OXC `Visit` helpers for cyclomatic complexity, nesting depth, JSX, hooks, `any`, console/debugger.

use std::collections::HashSet;

use oxc_ast::ast::*;
use oxc_ast::ast_kind::AstKind;
use oxc_ast_visit::walk;
use oxc_ast_visit::Visit;
use oxc_span::GetSpan;
use regex::Regex;

/// Cyclomatic-style complexity (base 1 + decision points), aligned with the historical TS scanner.
pub(crate) struct ComplexityVisitor {
    pub cc: usize,
}

impl<'a> Visit<'a> for ComplexityVisitor {
    fn enter_node(&mut self, kind: AstKind<'a>) {
        match kind {
            AstKind::IfStatement(_)
            | AstKind::ForStatement(_)
            | AstKind::ForInStatement(_)
            | AstKind::ForOfStatement(_)
            | AstKind::WhileStatement(_)
            | AstKind::DoWhileStatement(_)
            | AstKind::ConditionalExpression(_)
            | AstKind::CatchClause(_) => self.cc += 1,
            AstKind::SwitchCase(sc) => {
                if sc.test.is_some() {
                    self.cc += 1;
                }
            }
            AstKind::LogicalExpression(_) => self.cc += 1,
            _ => {}
        }
    }
}

pub(crate) fn complexity_function_body<'a>(body: &FunctionBody<'a>) -> usize {
    let mut v = ComplexityVisitor { cc: 1 };
    walk::walk_function_body(&mut v, body);
    v.cc
}

pub(crate) fn complexity_expression<'a>(expr: &Expression<'a>) -> usize {
    let mut v = ComplexityVisitor { cc: 1 };
    walk::walk_expression(&mut v, expr);
    v.cc
}

/// Max nesting depth, aligned with the historical TS scanner.
pub(crate) struct NestingVisitor {
    depth: u32,
    pub max_d: u32,
}

impl NestingVisitor {
    fn bump(&mut self) {
        self.depth += 1;
        self.max_d = self.max_d.max(self.depth);
    }
    fn pop(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }
}

impl<'a> Visit<'a> for NestingVisitor {
    fn enter_node(&mut self, kind: AstKind<'a>) {
        match kind {
            AstKind::IfStatement(_)
            | AstKind::ForStatement(_)
            | AstKind::ForInStatement(_)
            | AstKind::ForOfStatement(_)
            | AstKind::WhileStatement(_)
            | AstKind::DoWhileStatement(_)
            | AstKind::TryStatement(_)
            | AstKind::CatchClause(_)
            | AstKind::SwitchStatement(_) => self.bump(),
            _ => {}
        }
    }

    fn leave_node(&mut self, kind: AstKind<'a>) {
        match kind {
            AstKind::IfStatement(_)
            | AstKind::ForStatement(_)
            | AstKind::ForInStatement(_)
            | AstKind::ForOfStatement(_)
            | AstKind::WhileStatement(_)
            | AstKind::DoWhileStatement(_)
            | AstKind::TryStatement(_)
            | AstKind::CatchClause(_)
            | AstKind::SwitchStatement(_) => self.pop(),
            _ => {}
        }
    }
}

pub(crate) fn nesting_function_body<'a>(body: &FunctionBody<'a>) -> usize {
    let mut v = NestingVisitor { depth: 0, max_d: 0 };
    walk::walk_function_body(&mut v, body);
    v.max_d as usize
}

pub(crate) fn nesting_expression<'a>(expr: &Expression<'a>) -> usize {
    let mut v = NestingVisitor { depth: 0, max_d: 0 };
    walk::walk_expression(&mut v, expr);
    v.max_d as usize
}

pub(crate) struct JsxFinder(pub bool);

impl<'a> Visit<'a> for JsxFinder {
    fn enter_node(&mut self, kind: AstKind<'a>) {
        if matches!(
            kind,
            AstKind::JSXElement(_) | AstKind::JSXFragment(_) | AstKind::JSXOpeningElement(_)
        ) {
            self.0 = true;
        }
    }
}

pub(crate) fn body_contains_jsx<'a>(body: &FunctionBody<'a>) -> bool {
    let mut v = JsxFinder(false);
    walk::walk_function_body(&mut v, body);
    v.0
}

pub(crate) fn expr_contains_jsx<'a>(expr: &Expression<'a>) -> bool {
    let mut v = JsxFinder(false);
    walk::walk_expression(&mut v, expr);
    v.0
}

pub(crate) struct AnyCounter(pub usize);

impl<'a> Visit<'a> for AnyCounter {
    fn enter_node(&mut self, kind: AstKind<'a>) {
        if matches!(kind, AstKind::TSAnyKeyword(_)) {
            self.0 += 1;
        }
    }
}

pub(crate) fn count_any_in_program<'a>(program: &Program<'a>) -> usize {
    let mut v = AnyCounter(0);
    walk::walk_program(&mut v, program);
    v.0
}

pub(crate) struct HookCollector<'a> {
    pub hooks: Vec<&'a str>,
}

impl<'a> HookCollector<'a> {
    fn push_hook(&mut self, name: &'a str) {
        if name.len() > 3 && name.starts_with("use") {
            let rest = name.as_bytes().get(3).copied();
            if rest.is_some_and(|b| b.is_ascii_uppercase()) && !self.hooks.contains(&name) {
                self.hooks.push(name);
            }
        }
    }
}

impl<'a> Visit<'a> for HookCollector<'a> {
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        let name = call_callee_name(&it.callee);
        if let Some(n) = name {
            self.push_hook(n);
        }
        walk::walk_call_expression(self, it);
    }
}

fn call_callee_name<'a>(callee: &Expression<'a>) -> Option<&'a str> {
    match callee {
        Expression::Identifier(id) => Some(id.name.as_str()),
        Expression::StaticMemberExpression(sm) => Some(sm.property.name.as_str()),
        Expression::ChainExpression(chain) => chain_callee_name(&chain.expression),
        _ => None,
    }
}

fn chain_callee_name<'a>(el: &ChainElement<'a>) -> Option<&'a str> {
    match el {
        ChainElement::CallExpression(call) => call_callee_name(&call.callee),
        ChainElement::ComputedMemberExpression(_) => None,
        ChainElement::StaticMemberExpression(sm) => Some(sm.property.name.as_str()),
        ChainElement::PrivateFieldExpression(_) => None,
        ChainElement::TSNonNullExpression(nn) => {
            if let Expression::ChainExpression(inner) = &nn.expression {
                chain_callee_name(&inner.expression)
            } else {
                None
            }
        }
    }
}

pub(crate) fn collect_hooks_in_body<'a>(body: &FunctionBody<'a>) -> Vec<String> {
    let mut v = HookCollector { hooks: Vec::new() };
    walk::walk_function_body(&mut v, body);
    v.hooks.iter().map(|s| (*s).to_string()).collect()
}

pub(crate) fn collect_hooks_in_expr<'a>(expr: &Expression<'a>) -> Vec<String> {
    let mut v = HookCollector { hooks: Vec::new() };
    walk::walk_expression(&mut v, expr);
    v.hooks.iter().map(|s| (*s).to_string()).collect()
}

pub(crate) struct ConsoleDebuggerCollector<'a> {
    pub rel_file: &'a str,
    pub source: &'a str,
    pub out: Vec<crate::types::ConsoleDebuggerInfo>,
}

fn line_at(source: &str, offset: u32) -> usize {
    let o = offset as usize;
    if o > source.len() {
        return 1;
    }
    source[..o].bytes().filter(|&b| b == b'\n').count() + 1
}

impl<'a> Visit<'a> for ConsoleDebuggerCollector<'a> {
    fn visit_debugger_statement(&mut self, it: &DebuggerStatement) {
        self.out.push(crate::types::ConsoleDebuggerInfo {
            file: self.rel_file.to_string(),
            line: line_at(self.source, it.span.start),
            kind: "debugger".to_string(),
        });
        walk::walk_debugger_statement(self, it);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if let Expression::StaticMemberExpression(sm) = &it.callee {
            if let Expression::Identifier(obj) = &sm.object {
                if obj.name.as_str() == "console" {
                    let method = sm.property.name.as_str();
                    self.out.push(crate::types::ConsoleDebuggerInfo {
                        file: self.rel_file.to_string(),
                        line: line_at(self.source, it.span.start),
                        kind: format!("console.{method}"),
                    });
                }
            }
        }
        walk::walk_call_expression(self, it);
    }
}

pub(crate) fn collect_console_debugger<'a>(
    program: &Program<'a>,
    rel_file: &'a str,
    source: &'a str,
) -> Vec<crate::types::ConsoleDebuggerInfo> {
    let mut v = ConsoleDebuggerCollector {
        rel_file,
        source,
        out: Vec::new(),
    };
    walk::walk_program(&mut v, program);
    v.out
}

pub(crate) struct SilentCatchCollector<'a> {
    pub rel_file: &'a str,
    pub source: &'a str,
    pub out: Vec<crate::types::SilentCatchInfo>,
}

impl<'a> Visit<'a> for SilentCatchCollector<'a> {
    fn visit_try_statement(&mut self, it: &TryStatement<'a>) {
        if let Some(handler) = &it.handler {
            if handler.body.body.is_empty() {
                self.out.push(crate::types::SilentCatchInfo {
                    file: self.rel_file.to_string(),
                    line: line_at(self.source, handler.span.start),
                    kind: "empty catch clause".to_string(),
                });
            }
        }
        walk::walk_try_statement(self, it);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if it.arguments.len() == 1 && callee_chain_ends_with_catch(&it.callee) {
            let kind = match &it.arguments[0] {
                Argument::ArrowFunctionExpression(arr) => {
                    if arr.expression {
                        None
                    } else if arr.body.statements.is_empty() {
                        Some(".catch(() => {})".to_string())
                    } else {
                        None
                    }
                }
                Argument::FunctionExpression(fe) => fe
                    .body
                    .as_ref()
                    .filter(|b| b.statements.is_empty())
                    .map(|_| ".catch(function() {})".to_string()),
                _ => None,
            };
            if let Some(kind) = kind {
                self.out.push(crate::types::SilentCatchInfo {
                    file: self.rel_file.to_string(),
                    line: line_at(self.source, it.span.start),
                    kind,
                });
            }
        }
        walk::walk_call_expression(self, it);
    }
}

fn callee_chain_ends_with_catch<'a>(callee: &Expression<'a>) -> bool {
    match callee {
        Expression::StaticMemberExpression(sm) => sm.property.name.as_str() == "catch",
        Expression::ChainExpression(c) => chain_ends_with_catch(&c.expression),
        _ => false,
    }
}

fn chain_ends_with_catch<'a>(el: &ChainElement<'a>) -> bool {
    match el {
        ChainElement::StaticMemberExpression(sm) => sm.property.name.as_str() == "catch",
        ChainElement::CallExpression(call) => callee_chain_ends_with_catch(&call.callee),
        ChainElement::ComputedMemberExpression(_) => false,
        ChainElement::PrivateFieldExpression(_) => false,
        ChainElement::TSNonNullExpression(nn) => {
            matches!(&nn.expression, Expression::ChainExpression(inner) if chain_ends_with_catch(&inner.expression))
        }
    }
}

pub(crate) fn collect_silent_catches<'a>(
    program: &Program<'a>,
    rel_file: &'a str,
    source: &'a str,
) -> Vec<crate::types::SilentCatchInfo> {
    let mut v = SilentCatchCollector {
        rel_file,
        source,
        out: Vec::new(),
    };
    walk::walk_program(&mut v, program);
    v.out
}

static ORM_IGNORE: &[&str] = &[
    "toISOString",
    "getTime",
    "valueOf",
    "toString",
    "hasOwnProperty",
    "toLowerCase",
    "toUpperCase",
    "startsWith",
    "endsWith",
    "replaceAll",
];

struct OrmVisitor<'s> {
    source: &'s str,
    rel: &'s str,
    methods: &'s HashSet<String>,
    re: Regex,
    out: Vec<crate::types::OrmCaseFinding>,
}

impl<'s> OrmVisitor<'s> {
    fn scan_text(&mut self, text: &str, line: usize, method: &str) {
        for caps in self.re.captures_iter(text) {
            let id = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            if ORM_IGNORE.contains(&id) {
                continue;
            }
            self.out.push(crate::types::OrmCaseFinding {
                file: self.rel.to_string(),
                line,
                method: method.to_string(),
                snippet: caps
                    .get(0)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default(),
                identifier: id.to_string(),
            });
        }
    }
}

impl<'a, 's> Visit<'a> for OrmVisitor<'s> {
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if let Expression::StaticMemberExpression(sm) = &it.callee {
            let method = sm.property.name.as_str();
            if self.methods.contains(method) {
                for arg in &it.arguments {
                    let line = line_at(self.source, arg.span().start);
                    match arg {
                        Argument::StringLiteral(s) => {
                            self.scan_text(s.value.as_str(), line, method);
                        }
                        Argument::TemplateLiteral(t) => {
                            for q in &t.quasis {
                                self.scan_text(q.value.raw.as_str(), line, method);
                                if let Some(cooked) = q.value.cooked.as_ref() {
                                    self.scan_text(cooked.as_str(), line, method);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        walk::walk_call_expression(self, it);
    }
}

pub(crate) fn collect_orm_case_findings<'a>(
    program: &Program<'a>,
    source: &str,
    rel: &str,
    methods: &HashSet<String>,
) -> Vec<crate::types::OrmCaseFinding> {
    let re = Regex::new(r"\.([a-z][a-zA-Z0-9]*[A-Z][a-zA-Z0-9]*)\b")
        .expect("static camelCase segment regex for ORM string check");
    let mut v = OrmVisitor {
        source,
        rel,
        methods,
        re,
        out: Vec::new(),
    };
    walk::walk_program(&mut v, program);
    v.out
}
