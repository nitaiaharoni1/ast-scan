use std::collections::{BTreeSet, HashMap, HashSet};

const WHITE: u8 = 0;
const GRAY: u8 = 1;
const BLACK: u8 = 2;

/// DFS-based cycle detection in a directed graph. Returns all cycles found.
pub(crate) fn find_cycles(graph: &HashMap<String, HashSet<String>>) -> Vec<Vec<String>> {
    let mut color: HashMap<&str, u8> = HashMap::new();
    let mut path: Vec<&str> = Vec::new();
    let mut cycles: Vec<Vec<String>> = Vec::new();

    fn dfs<'a>(
        u: &'a str,
        graph: &'a HashMap<String, HashSet<String>>,
        color: &mut HashMap<&'a str, u8>,
        path: &mut Vec<&'a str>,
        cycles: &mut Vec<Vec<String>>,
    ) {
        color.insert(u, GRAY);
        path.push(u);

        if let Some(neighbors) = graph.get(u) {
            let sorted: BTreeSet<&String> = neighbors.iter().collect();
            for v in sorted {
                let vc = color.get(v.as_str()).copied().unwrap_or(WHITE);
                if vc == GRAY {
                    let idx = path
                        .iter()
                        .position(|&n| n == v.as_str())
                        .expect("gray node must be on DFS path");
                    let mut cycle: Vec<String> =
                        path[idx..].iter().map(|s| s.to_string()).collect();
                    cycle.push(v.clone());
                    cycles.push(cycle);
                } else if vc == WHITE {
                    dfs(v, graph, color, path, cycles);
                }
            }
        }

        path.pop();
        color.insert(u, BLACK);
    }

    let mut keys: Vec<&String> = graph.keys().collect();
    keys.sort();
    for node in keys {
        if color.get(node.as_str()).copied().unwrap_or(WHITE) == WHITE {
            dfs(node, graph, &mut color, &mut path, &mut cycles);
        }
    }

    cycles
}

/// Dedup cycles by treating each cycle as an unordered set of nodes (minus the repeated tail).
pub(crate) fn unique_cycles(cycles: &[Vec<String>]) -> Vec<Vec<String>> {
    let mut seen: HashSet<BTreeSet<String>> = HashSet::new();
    let mut unique: Vec<Vec<String>> = Vec::new();

    for c in cycles {
        let key: BTreeSet<String> = c[..c.len() - 1].iter().cloned().collect();
        if seen.insert(key) {
            unique.push(c.clone());
        }
    }

    unique
}

/// Coupling row: module name, afferent (Ca), efferent (Ce), instability.
pub(crate) struct CouplingRow {
    pub module: String,
    pub ca: usize,
    pub ce: usize,
    pub instability: f64,
}

/// Compute Ca/Ce/instability for each module that has at least one edge.
/// Sorted by total (Ca+Ce) descending, then by module name.
pub(crate) fn compute_coupling(
    graph: &HashMap<String, HashSet<String>>,
    all_modules: &HashSet<String>,
) -> Vec<CouplingRow> {
    let mut afferent: HashMap<String, usize> = HashMap::new();
    let mut efferent: HashMap<String, usize> = HashMap::new();
    for (src, targets) in graph {
        efferent.insert(src.clone(), targets.len());
        for tgt in targets {
            *afferent.entry(tgt.clone()).or_insert(0) += 1;
        }
    }

    let mut rows: Vec<CouplingRow> = Vec::new();
    for m in all_modules {
        let ca = *afferent.get(m).unwrap_or(&0);
        let ce = *efferent.get(m).unwrap_or(&0);
        let total = ca + ce;
        if total > 0 {
            let instability = ((ce as f64 / total as f64) * 100.0).round() / 100.0;
            rows.push(CouplingRow {
                module: m.clone(),
                ca,
                ce,
                instability,
            });
        }
    }
    rows.sort_by(|a, b| {
        (b.ca + b.ce)
            .cmp(&(a.ca + a.ce))
            .then_with(|| a.module.cmp(&b.module))
    });
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_graph(edges: &[(&str, &str)]) -> HashMap<String, HashSet<String>> {
        let mut g: HashMap<String, HashSet<String>> = HashMap::new();
        for &(src, dst) in edges {
            g.entry(src.to_string())
                .or_default()
                .insert(dst.to_string());
            g.entry(dst.to_string()).or_default();
        }
        g
    }

    #[test]
    fn test_simple_cycle() {
        let g = make_graph(&[("a", "b"), ("b", "c"), ("c", "a")]);
        let cycles = find_cycles(&g);
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0], vec!["a", "b", "c", "a"]);
    }

    #[test]
    fn test_no_cycle() {
        let g = make_graph(&[("a", "b"), ("b", "c")]);
        let cycles = find_cycles(&g);
        assert!(cycles.is_empty());
    }

    #[test]
    fn test_unique_dedup() {
        let cycles = vec![
            vec!["a".into(), "b".into(), "a".into()],
            vec!["b".into(), "a".into(), "b".into()],
        ];
        let u = unique_cycles(&cycles);
        assert_eq!(u.len(), 1);
    }

    #[test]
    fn test_coupling_basic() {
        let g = make_graph(&[("a", "b"), ("a", "c"), ("b", "c")]);
        let all: HashSet<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        let rows = compute_coupling(&g, &all);
        assert_eq!(rows.len(), 3);
        let a = rows.iter().find(|r| r.module == "a").unwrap();
        assert_eq!(a.ca, 0);
        assert_eq!(a.ce, 2);
        assert!((a.instability - 1.0).abs() < 0.01);
        let c = rows.iter().find(|r| r.module == "c").unwrap();
        assert_eq!(c.ca, 2);
        assert_eq!(c.ce, 0);
        assert!((c.instability - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_coupling_empty_graph() {
        let g = HashMap::new();
        let all: HashSet<String> = ["x", "y"].iter().map(|s| s.to_string()).collect();
        let rows = compute_coupling(&g, &all);
        assert!(rows.is_empty());
    }

    #[test]
    fn test_coupling_sorted_by_total() {
        let g = make_graph(&[("a", "b"), ("c", "d"), ("c", "e"), ("c", "b")]);
        let all: HashSet<String> = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let rows = compute_coupling(&g, &all);
        assert!(rows[0].ca + rows[0].ce >= rows[1].ca + rows[1].ce);
    }
}
