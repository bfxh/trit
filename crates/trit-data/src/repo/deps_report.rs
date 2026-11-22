//! 依赖图统计：边、外部依赖、本地未解析、测试关联、环上文件。

use std::collections::{BTreeMap, VecDeque};

use crate::repo::Unit;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub edges: usize,
    pub imports_resolved: usize,
    pub imports_external: usize,
    pub imports_unresolved: usize,
    pub imports_dynamic: usize,
    pub tested_sources: usize,
    pub test_links: usize,
    pub cyclic: usize,
}

pub fn summarize(units: &[Unit]) -> Summary {
    let edges: usize = units.iter().map(|u| u.imports_resolved.len()).sum();
    Summary {
        edges,
        imports_resolved: edges,
        imports_external: units.iter().map(|u| u.imports_external.len()).sum(),
        imports_unresolved: units.iter().map(|u| u.unresolved.len()).sum(),
        imports_dynamic: units.iter().map(|u| u.imports_dynamic.len()).sum(),
        tested_sources: units
            .iter()
            .filter(|u| !u.is_test && !u.tests.is_empty())
            .count(),
        test_links: units.iter().map(|u| u.tests_for.len()).sum(),
        cyclic: cyclic_nodes(units),
    }
}

/// Kahn 拓扑排序后剩下的节点：落在环上或只被环依赖的文件数。
fn cyclic_nodes(units: &[Unit]) -> usize {
    let mut indeg: BTreeMap<&str, usize> = units.iter().map(|u| (u.path.as_str(), 0)).collect();
    for u in units {
        for t in &u.imports_resolved {
            if let Some(d) = indeg.get_mut(t.as_str()) {
                *d += 1;
            }
        }
    }
    let mut q: VecDeque<&str> = indeg
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(p, _)| *p)
        .collect();
    let mut removed = 0;
    while let Some(p) = q.pop_front() {
        removed += 1;
        if let Some(u) = units.iter().find(|u| u.path == p) {
            for t in &u.imports_resolved {
                if let Some(d) = indeg.get_mut(t.as_str()) {
                    *d -= 1;
                    if *d == 0 {
                        q.push_back(t.as_str());
                    }
                }
            }
        }
    }
    units.len() - removed
}
