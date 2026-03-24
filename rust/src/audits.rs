use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;

fn todo_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(concat!(
            r"(?i)(?://|/\*|#).*\b(",
            "TO",
            "DO",
            "|FI",
            "XME",
            "|HA",
            "CK",
            "|XX",
            "X",
            r")\b"
        ))
        .expect("static todo-marker regex")
    })
}

fn eslint_disable_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?://|/\*)\s*eslint-disable(?:-next-line)?\s+([^\n*]+)")
            .expect("static eslint-disable regex")
    })
}

fn ts_directive_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(concat!(
            r"//\s*(@ts-ignore|@ts-expect-",
            "error",
            r"|@ts-nocheck)\b"
        ))
        .expect("static ts-directive regex")
    })
}

/// Scan source text for task-marker comments (tags defined in `todo_re`).
/// Updates `freq` (tag -> count) and `samples` (tag -> up to 5 file locations).
pub(crate) fn collect_todo_comments(
    source: &str,
    rel_file: &str,
    freq: &mut HashMap<String, usize>,
    samples: &mut HashMap<String, Vec<String>>,
) {
    let re = todo_re();
    for (lineno, line) in source.lines().enumerate() {
        if let Some(caps) = re.captures(line) {
            let tag = caps[1].to_uppercase();
            *freq.entry(tag.clone()).or_insert(0) += 1;
            let list = samples.entry(tag).or_default();
            let loc = format!("{}:{}", rel_file, lineno + 1);
            if list.len() < 5 && !list.contains(&loc) {
                list.push(loc);
            }
        }
    }
}

/// Scan for `eslint-disable` / `eslint-disable-next-line` comments and extract rule names.
/// Updates `map`: rule -> (count, sample_files up to 5).
pub(crate) fn collect_eslint_disables(
    source: &str,
    rel_file: &str,
    map: &mut HashMap<String, (usize, Vec<String>)>,
) {
    let re = eslint_disable_re();
    for caps in re.captures_iter(source) {
        let chunk = &caps[1];
        for raw in chunk.split(',') {
            let rule = raw.split_whitespace().next().unwrap_or("");
            if rule.is_empty()
                || rule.starts_with("--")
                || rule == "eslint-disable"
                || rule == "eslint-enable"
            {
                continue;
            }
            let entry = map
                .entry(rule.to_string())
                .or_insert_with(|| (0, Vec::new()));
            entry.0 += 1;
            if entry.1.len() < 5 && !entry.1.contains(&rel_file.to_string()) {
                entry.1.push(rel_file.to_string());
            }
        }
    }
}

/// Scan for `@ts-ignore` / `@ts-expect-error` / `@ts-nocheck` directives.
/// Updates `map`: directive -> (count, sample_files up to 5).
pub(crate) fn collect_ts_directives(
    source: &str,
    rel_file: &str,
    map: &mut HashMap<String, (usize, Vec<String>)>,
) {
    let re = ts_directive_re();
    for caps in re.captures_iter(source) {
        let directive = caps[1].to_string();
        let entry = map.entry(directive).or_insert_with(|| (0, Vec::new()));
        entry.0 += 1;
        if entry.1.len() < 5 && !entry.1.contains(&rel_file.to_string()) {
            entry.1.push(rel_file.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_todo_comments() {
        let source = concat!(
            "// TO", "DO: fix this\n",
            "let x = 1;\n",
            "/* FI", "XME later */\n",
            "# HA", "CK workaround",
        );
        let mut freq = HashMap::new();
        let mut samples = HashMap::new();
        collect_todo_comments(source, "file.ts", &mut freq, &mut samples);
        assert_eq!(freq.get("TODO"), Some(&1));
        assert_eq!(freq.get("FIXME"), Some(&1));
        assert_eq!(freq.get("HACK"), Some(&1));
        assert_eq!(samples["TODO"], vec!["file.ts:1"]);
    }

    #[test]
    fn test_todo_comments_rust_doc_style() {
        let source = concat!(
            "//! FI", "XME: module note\n",
            "/// TO", "DO: item doc\n",
            "fn main() {}",
        );
        let mut freq = HashMap::new();
        let mut samples = HashMap::new();
        collect_todo_comments(source, "lib.rs", &mut freq, &mut samples);
        assert_eq!(freq.get("FIXME"), Some(&1));
        assert_eq!(freq.get("TODO"), Some(&1));
        assert_eq!(samples["TODO"], vec!["lib.rs:2"]);
    }

    #[test]
    fn test_eslint_disables() {
        let source = "// eslint-disable-next-line no-console, no-debugger\nfoo();";
        let mut map = HashMap::new();
        collect_eslint_disables(source, "a.ts", &mut map);
        assert_eq!(map.get("no-console").unwrap().0, 1);
        assert_eq!(map.get("no-debugger").unwrap().0, 1);
    }

    #[test]
    fn test_ts_directives() {
        let source = "// @ts-ignore\nlet x: any = 1;\n// @ts-expect-error\nfoo();";
        let mut map = HashMap::new();
        collect_ts_directives(source, "b.ts", &mut map);
        assert_eq!(map.get("@ts-ignore").unwrap().0, 1);
        assert_eq!(map.get("@ts-expect-error").unwrap().0, 1);
    }

    #[test]
    fn test_eslint_skips_dashes() {
        let source = "// eslint-disable no-console -- reason here";
        let mut map = HashMap::new();
        collect_eslint_disables(source, "c.ts", &mut map);
        assert_eq!(map.get("no-console").unwrap().0, 1);
        assert!(!map.contains_key("--"));
        assert!(!map.contains_key("reason"));
    }
}
