//! Exact Boolean model counting by DPLL with component caching.
//!
//! The solver is deliberately small: unit propagation, connected-component
//! decomposition, a canonical residual key, and a memo table.  It is not a
//! competing SAT solver; it is the counting reference for the prototype.

use std::collections::HashMap;

use crate::cnf::Cnf;

/// Solver counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CountStats {
    /// Recursive nodes visited.
    pub nodes: u64,
    /// Cache lookups that hit.
    pub cache_hits: u64,
    /// Cache lookups that missed.
    pub cache_misses: u64,
    /// Nodes split into more than one component.
    pub components: u64,
    /// Branch decisions.
    pub branches: u64,
}

/// Result of counting a CNF.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CountResult {
    /// Number of satisfying Boolean assignments.
    pub count: u128,
    /// Solver counters.
    pub stats: CountStats,
}

/// Counts all satisfying assignments of `cnf`.
#[must_use]
pub fn count_cnf(cnf: &Cnf) -> CountResult {
    let clauses = normalize(&cnf.clauses, cnf.nvars);
    if clauses.iter().any(Vec::is_empty) {
        return CountResult::default();
    }
    let mut counter = Counter {
        cache: HashMap::new(),
        stats: CountStats::default(),
    };
    let count = counter.count_core(&clauses, cnf.nvars);
    CountResult {
        count,
        stats: counter.stats,
    }
}

struct Counter {
    cache: HashMap<Vec<i32>, u128>,
    stats: CountStats,
}

impl Counter {
    fn count_core(&mut self, clauses: &[Vec<i32>], k: usize) -> u128 {
        self.stats.nodes += 1;
        let clauses = normalize(clauses, k);
        if clauses.iter().any(Vec::is_empty) {
            return 0;
        }

        let (assign, conflict) = propagate(&clauses, k);
        if conflict {
            return 0;
        }

        let mut residual = Vec::new();
        let mut occurring = vec![false; k + 1];
        for clause in &clauses {
            let mut satisfied = false;
            let mut open = Vec::with_capacity(clause.len());
            for &lit in clause {
                let var = lit.unsigned_abs() as usize;
                match assign[var] {
                    0 => open.push(lit),
                    1 if lit > 0 => {
                        satisfied = true;
                        break;
                    }
                    -1 if lit < 0 => {
                        satisfied = true;
                        break;
                    }
                    _ => {}
                }
            }
            if satisfied {
                continue;
            }
            if open.is_empty() {
                return 0;
            }
            for &lit in &open {
                occurring[lit.unsigned_abs() as usize] = true;
            }
            residual.push(open);
        }

        let free = (1..=k).filter(|&v| assign[v] == 0 && !occurring[v]).count();
        debug_assert!(free < 128, "free variable count too large for u128");
        let free_factor = 1u128
            .checked_shl(free as u32)
            .expect("free variable count exceeds u128 capacity");

        if residual.is_empty() {
            return free_factor;
        }

        let vars: Vec<usize> = (1..=k).filter(|&v| occurring[v]).collect();
        let m = vars.len();
        let renumbered = renumber(&residual, &vars);

        if let Some(parts) = decompose(&renumbered, m) {
            self.stats.components += 1;
            let mut product = free_factor;
            for (part_vars, part_clauses) in parts {
                let local = renumber(&part_clauses, &part_vars);
                product *= self.count_core(&local, part_vars.len());
            }
            return product;
        }

        let key = cache_key(&renumbered, m);
        if let Some(cached) = self.cache.get(&key) {
            self.stats.cache_hits += 1;
            return free_factor * cached;
        }
        self.stats.cache_misses += 1;

        let branch = choose_var(&renumbered, m);
        self.stats.branches += 1;
        let mut positive = renumbered.clone();
        positive.push(vec![branch as i32]);
        let mut negative = renumbered;
        negative.push(vec![-(branch as i32)]);

        let total = self.count_core(&positive, m) + self.count_core(&negative, m);
        self.cache.insert(key, total);
        free_factor * total
    }
}

/// Sorts, deduplicates, and drops tautological clauses.
fn normalize(clauses: &[Vec<i32>], k: usize) -> Vec<Vec<i32>> {
    let mut out = Vec::with_capacity(clauses.len());
    for clause in clauses {
        let mut lits = clause.clone();
        lits.sort_unstable_by_key(|lit| (lit.unsigned_abs(), *lit < 0));
        lits.dedup();
        debug_assert!(lits.iter().all(|lit| lit.unsigned_abs() as usize <= k));
        if lits.windows(2).any(|pair| pair[0] == -pair[1]) {
            continue;
        }
        out.push(lits);
    }
    out.sort();
    out
}

/// Unit propagation. Returns `(assignment, conflict)`.
fn propagate(clauses: &[Vec<i32>], k: usize) -> (Vec<i8>, bool) {
    let mut assign = vec![0i8; k + 1];
    loop {
        let mut changed = false;
        for clause in clauses {
            let mut satisfied = false;
            let mut unassigned = 0usize;
            let mut last = 0i32;
            for &lit in clause {
                let var = lit.unsigned_abs() as usize;
                match assign[var] {
                    0 => {
                        unassigned += 1;
                        last = lit;
                    }
                    1 if lit > 0 => {
                        satisfied = true;
                        break;
                    }
                    -1 if lit < 0 => {
                        satisfied = true;
                        break;
                    }
                    _ => {}
                }
            }
            if satisfied {
                continue;
            }
            if unassigned == 0 {
                return (assign, true);
            }
            if unassigned == 1 {
                let var = last.unsigned_abs() as usize;
                let value = if last > 0 { 1 } else { -1 };
                if assign[var] == 0 {
                    assign[var] = value;
                    changed = true;
                } else if assign[var] != value {
                    return (assign, true);
                }
            }
        }
        if !changed {
            return (assign, false);
        }
    }
}

/// Maps `vars` to `1..=vars.len()` and rewrites clauses.
fn renumber(clauses: &[Vec<i32>], vars: &[usize]) -> Vec<Vec<i32>> {
    let max = vars.iter().copied().max().unwrap_or(0);
    let mut map = vec![0i32; max + 1];
    for (index, &var) in vars.iter().enumerate() {
        map[var] = (index + 1) as i32;
    }
    let mut out: Vec<Vec<i32>> = clauses
        .iter()
        .map(|clause| {
            clause
                .iter()
                .map(|&lit| {
                    let var = lit.unsigned_abs() as usize;
                    let mapped = map[var];
                    debug_assert!(mapped != 0);
                    if lit > 0 { mapped } else { -mapped }
                })
                .collect()
        })
        .collect();
    out.sort();
    out
}

/// A connected component: its variables and clauses.
type Component = (Vec<usize>, Vec<Vec<i32>>);

/// Splits a CNF into connected components when there is more than one.
fn decompose(clauses: &[Vec<i32>], k: usize) -> Option<Vec<Component>> {
    if k == 0 {
        return None;
    }
    let mut parent: Vec<usize> = (0..=k).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    fn union(parent: &mut [usize], a: usize, b: usize) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra != rb {
            parent[ra] = rb;
        }
    }
    for clause in clauses {
        let first = clause[0].unsigned_abs() as usize;
        for lit in &clause[1..] {
            union(&mut parent, first, lit.unsigned_abs() as usize);
        }
    }

    let mut groups: HashMap<usize, (Vec<usize>, Vec<Vec<i32>>)> = HashMap::new();
    for clause in clauses {
        let root = find(&mut parent, clause[0].unsigned_abs() as usize);
        groups
            .entry(root)
            .or_insert_with(|| (Vec::new(), Vec::new()))
            .1
            .push(clause.clone());
    }
    if groups.len() <= 1 {
        return None;
    }
    let mut out = Vec::with_capacity(groups.len());
    for (_, (_, group_clauses)) in groups {
        let mut vars: Vec<usize> = group_clauses
            .iter()
            .flat_map(|clause| clause.iter().map(|lit| lit.unsigned_abs() as usize))
            .collect();
        vars.sort_unstable();
        vars.dedup();
        out.push((vars, group_clauses));
    }
    Some(out)
}

/// Picks the most frequent variable; ties go to the lowest index.
fn choose_var(clauses: &[Vec<i32>], k: usize) -> usize {
    let mut score = vec![0u32; k + 1];
    for clause in clauses {
        let weight = 1u32 << (10 - clause.len().min(10));
        for &lit in clause {
            score[lit.unsigned_abs() as usize] += weight;
        }
    }
    (1..=k)
        .max_by_key(|&var| (score[var], std::cmp::Reverse(var)))
        .expect("nonempty component")
}

/// Canonical cache key: variable count, clause count, then clauses.
fn cache_key(clauses: &[Vec<i32>], k: usize) -> Vec<i32> {
    let mut key = Vec::with_capacity(2 + clauses.iter().map(|c| c.len() + 1).sum::<usize>());
    key.push(k as i32);
    key.push(clauses.len() as i32);
    for clause in clauses {
        key.push(clause.len() as i32);
        key.extend_from_slice(clause);
    }
    key
}
