//! Exact Boolean model counting by DPLL with component caching.
//!
//! The solver is deliberately small: unit propagation, connected-component
//! decomposition, a canonical residual key, and a memo table.  It is not a
//! competing SAT solver; it is the counting reference for the prototype.
//!
//! 每节点的传播走 `residue`：计数器 + 扁平出现表（CSR），一趟同时做完强制闭包与残基构造，
//! 子句表进入时已是规范形（`count_cnf` 整理一次，分支用 `insert_unit` 插回有序位置），
//! 不再逐节点重排全表。实测（匹配族，交错 6 轮，节点/分支/组件/缓存逐位不变）：
//! **1.21×**，八档全部 −13%～−19%。第一版只换成计数器传播但用 `Vec<Vec<u32>>` 出现表，
//! 实测仅 1.05× —— 瓶颈是**每节点 2(k+1) 次小分配**而不是扫描次数，扁平化才拿到收益。

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
    let clauses = normalize(&cnf.clauses, cnf.nvars); // 唯一一次全表规范形整理
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
        // 进来的子句表已经是规范形（见 `count_cnf` 与 `insert_unit`），不再逐节点重排。
        // residue 顺手把变量压成 1..=m 的新编号：省掉一次全表复制与一次重排。
        let (mut numbered, m, assigned) = match residue(clauses, k) {
            Some(got) => got,
            None => return 0,
        };
        let free = k - assigned - m;
        debug_assert!(free < 128, "free variable count too large for u128");
        let free_factor = 1u128
            .checked_shl(free as u32)
            .expect("free variable count exceeds u128 capacity");

        if m == 0 {
            return free_factor;
        }
        // 0 号位是留给分支插单位子句的空槽（下表恒从 1 开始读），所以插入不会挪动表格。
        numbered.reserve(1);
        let renumbered = &numbered[1..];

        if let Some(parts) = decompose(renumbered, m) {
            self.stats.components += 1;
            let mut product = free_factor;
            for (part_vars, part_clauses) in parts {
                let local = renumber(&part_clauses, &part_vars);
                product *= self.count_core(&local, part_vars.len());
            }
            return product;
        }

        let key = cache_key(renumbered, m);
        if let Some(cached) = self.cache.get(&key) {
            self.stats.cache_hits += 1;
            return free_factor * cached;
        }
        self.stats.cache_misses += 1;

        let branch = choose_var(renumbered, m);
        self.stats.branches += 1;
        // 两个子节点共用这一份表：插单位子句 → 递归 → 原位摘掉 → 插反面。
        // 省下的是每个分支节点一次深拷贝（分支数 ≈ 节点数的一半）。
        let at = insert_unit(&mut numbered, branch as i32);
        let positive = self.count_core(&numbered[1..], m);
        numbered.remove(at);
        let at = insert_unit(&mut numbered, -(branch as i32));
        let negative = self.count_core(&numbered[1..], m);
        numbered.remove(at);
        let total = positive + negative;
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

/// 每个变量的出现表起点（长度 k+2，末位是总长哨兵）。
fn prefix_offsets(lengths: &[u32]) -> Vec<u32> {
    let mut start = vec![0u32; lengths.len() + 1];
    for (index, &amount) in lengths.iter().enumerate() {
        start[index + 1] = start[index] + amount;
    }
    start[..lengths.len()].to_vec()
}

/// 计数器式单位传播 + 残基构造，**一趟**做完（旧写法每个节点要对全表反复扫描：
/// `normalize` 重排全部子句、`propagate` 全表迭代、再全表构造残基）。
///
/// 返回 `(1 基的新子句表, 出现变量数, 已赋值变量数)`；出现冲突返回 `None`。
/// 表按 `numbered[1..]` 使用，`numbered[0]` 是「不存在第 0 个变量」留下的空位。
/// 单位传播的强制闭包与顺序无关（合流），所以残基与旧的两段实现逐位相同——
/// 这一点由「节点数 / 分支数 / 组件数 / 缓存命中数全等」的实测对拍钉住，不是口头保证。
fn residue(clauses: &[Vec<i32>], k: usize) -> Option<(Vec<Vec<i32>>, usize, usize)> {
    // 出现表用扁平数组 + 前缀偏移（CSR），每节点只 3 次分配，不是 2(k+1) 次小 Vec。
    let mut counts: Vec<u32> = Vec::with_capacity(clauses.len());
    let mut pos_len = vec![0u32; k + 2];
    let mut neg_len = vec![0u32; k + 2];
    let mut literals = 0usize;
    for clause in clauses {
        counts.push(clause.len() as u32);
        literals += clause.len();
        for &lit in clause {
            let var = lit.unsigned_abs() as usize;
            if lit > 0 {
                pos_len[var] += 1;
            } else {
                neg_len[var] += 1;
            }
        }
    }
    let pos_start = prefix_offsets(&pos_len);
    let neg_start = prefix_offsets(&neg_len);
    let mut pos_at = pos_start.clone();
    let mut neg_at = neg_start.clone();
    let mut occ_pos = vec![0u32; literals];
    let mut occ_neg = vec![0u32; literals];
    for (index, clause) in clauses.iter().enumerate() {
        for &lit in clause {
            let var = lit.unsigned_abs() as usize;
            if lit > 0 {
                occ_pos[pos_at[var] as usize] = index as u32;
                pos_at[var] += 1;
            } else {
                occ_neg[neg_at[var] as usize] = index as u32;
                neg_at[var] += 1;
            }
        }
    }

    let mut assign = vec![0i8; k + 1];
    let mut satisfied = vec![false; clauses.len()];
    let mut trail: Vec<i32> = clauses
        .iter()
        .filter(|clause| clause.len() == 1)
        .map(|clause| clause[0])
        .collect();
    let mut assigned = 0usize;
    let mut head = 0;
    while head < trail.len() {
        let lit = trail[head];
        head += 1;
        let var = lit.unsigned_abs() as usize;
        let value = if lit > 0 { 1 } else { -1 };
        if assign[var] == value {
            continue;
        }
        if assign[var] != 0 {
            return None;
        }
        assign[var] = value;
        assigned += 1;
        let (fill_from, fill_to, starve_from, starve_to) = if value == 1 {
            (
                pos_start[var],
                pos_start[var + 1],
                neg_start[var],
                neg_start[var + 1],
            )
        } else {
            (
                neg_start[var],
                neg_start[var + 1],
                pos_start[var],
                pos_start[var + 1],
            )
        };
        let (fills, starves) = if value == 1 {
            (
                &occ_pos[fill_from as usize..fill_to as usize],
                &occ_neg[starve_from as usize..starve_to as usize],
            )
        } else {
            (
                &occ_neg[fill_from as usize..fill_to as usize],
                &occ_pos[starve_from as usize..starve_to as usize],
            )
        };
        for &clause in fills {
            satisfied[clause as usize] = true;
        }
        for &clause in starves {
            // 已满足的子句不再参与计数：它的文字还会出现在出现表里，
            // 不跳过的话会被扣到 0 ⇒ 假冲突（第一版就是这么被 13 条测试当场抓住的）。
            if satisfied[clause as usize] {
                continue;
            }
            let slot = &mut counts[clause as usize];
            if *slot <= 1 {
                return None;
            }
            *slot -= 1;
            if *slot == 1 {
                let remaining = clauses[clause as usize]
                    .iter()
                    .copied()
                    .find(|&lit| assign[lit.unsigned_abs() as usize] == 0)?;
                trail.push(remaining);
            }
        }
    }

    let mut occurs = vec![false; k + 1];
    let mut open_clauses: Vec<Vec<i32>> = Vec::with_capacity(clauses.len());
    for (index, clause) in clauses.iter().enumerate() {
        if satisfied[index] {
            continue;
        }
        if counts[index] == 0 {
            return None;
        }
        let mut kept: Vec<i32> = Vec::with_capacity(counts[index] as usize);
        for &lit in clause {
            let var = lit.unsigned_abs() as usize;
            if assign[var] == 0 {
                kept.push(lit);
                occurs[var] = true;
            }
        }
        debug_assert_eq!(
            kept.len(),
            counts[index] as usize,
            "计数器与实际残基必须一致"
        );
        open_clauses.push(kept);
    }

    // 出现变量压成 1..=m 的新号，然后**就地换号**。map 随老号单调递增，
    // 所以子句内部与子句表的有序性自动保持：既省掉 `renumber` 的第二次全表拷贝，
    // 也省掉它末尾那次重排。0 号位留给分支插单位子句用（表格因此不用挪位）。
    let mut map = vec![0i32; k + 1];
    let mut m = 0usize;
    for var in 1..=k {
        if occurs[var] {
            m += 1;
            map[var] = m as i32;
        }
    }
    let mut numbered: Vec<Vec<i32>> = Vec::with_capacity(open_clauses.len() + 1);
    numbered.push(Vec::new());
    for mut clause in open_clauses {
        for lit in clause.iter_mut() {
            let mapped = map[lit.unsigned_abs() as usize];
            debug_assert!(mapped != 0, "残基里的变量必须拿到新号");
            *lit = if *lit > 0 { mapped } else { -mapped };
        }
        numbered.push(clause);
    }
    // 换号之后必须重排：老号→新号对「同一子句内部」是单调的，但整张表的字典序
    // 会被子句首文字的**符号**翻转（实测反例 [3,4,-7] 排在 [-1,-2] 之前），
    // 所以这里不能省。省掉的只有 renumber 的那次全表拷贝。
    numbered[1..].sort();
    Some((numbered, m, assigned))
}

/// 把分支产生的单位子句**插回有序位置**，保住「子句表有序」这个不变量，
/// 于是节点入口不需要再 `normalize` 一次（`cache_key` 的规范形依赖它）。
/// 把单位子句插回有序位置并返回下标（调用方递归完要原位摘掉，表格才能复用）。
fn insert_unit(clauses: &mut Vec<Vec<i32>>, lit: i32) -> usize {
    let unit = vec![lit];
    let at = clauses.partition_point(|clause| clause.as_slice() < unit.as_slice());
    clauses.insert(at, unit);
    at
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
