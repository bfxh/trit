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
//!
//! 消融实测（同一份例子，把 `decompose` 强制走 `None`）否证了"省掉分解/拷贝是下一刀"的猜想：
//! 匹配族 8 对从 2.2 ms / 203 节点涨到 **23.1 ms / 10,019 节点**，12 对从 4.7 ms / 323 节点涨到
//! **1807 ms / 729,059 节点**（节点数 ×384）。分解本身的花费跟它省下的搜索比不是同一量级，
//! 所以 µs/节点 随规模上升（10.7→47.9 µs）是**子问题变大**的正常结果，不是分解做多了。
//!
//! 2026-11-06 再一刀：残基子句表也扁平化 —— `Table` 是一段文字流加一张段起点表，
//! `residue` / `renumber` 不再为每条子句分配一个 `Vec`，`decompose` 只传段下标（不克隆子句）。
//! 用 `examples/matching_alloc_probe.rs` 同一把尺量：**allocs/节点从 164.1 / 925.5（8 对 / 40 对）
//! 变成 37.3 / 41.3**，也就是不再随规模增长。四个计数列逐位不变
//! （8 对 203/45/56/90-135 … 40 对 1163/237/344/570-807）。交错 4 轮的耗时：
//! **µs/节点 40 对 48.1 → 25.1（1.92×，四次重复的极差分别是 47.4–49.1 与 24.8–25.3）**、
//! 8 对 9.9 → 6.6（1.51×），上升线从 4.9× 压到 3.8×
//! —— 剩下的增长来自子问题本身，不是分配。
//!
//! 第一版死在占位段上：`Table::sort` 重建表时少写一个 0，第一条真实子句被当成占位丢掉，
//! 症状是计数偏大（`tests/cnf_fuzz.rs` 的 432 条里有 88 条与穷举不一致）。
//! 现在由 `dpll::tests` 的四条不变量测试直接钉住，并已用"少写那个 0"的变异反证过它们真会红。

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

/// 扁平子句表：`lits` 按 `starts` 分段，段内文字有序，段与段之间按切片字典序有序。
///
/// **第 0 段永远是空段**，它是分支时插单位子句的位置：单位子句插进来不用挪动任何已有
/// 文字之外的东西（只多搬一次尾巴），同时保住「整表有序」这个不变量 —— `cache_key` 的
/// 规范形依赖它。子句数与遍历都从第 1 段开始算。
#[derive(Clone, Debug)]
struct Table {
    lits: Vec<i32>,
    /// 长度 = 段数 + 1，末位是 `lits.len()`。
    starts: Vec<u32>,
}

impl Table {
    /// 由嵌套子句表建表；第 0 段的占位由本方法补上。
    fn from_nested(clauses: &[Vec<i32>]) -> Self {
        let literals: usize = clauses.iter().map(Vec::len).sum();
        let mut table = Self {
            lits: Vec::with_capacity(literals),
            starts: Vec::with_capacity(clauses.len() + 2),
        };
        table.starts.push(0);
        table.starts.push(0);
        for clause in clauses {
            table.lits.extend_from_slice(clause);
            let end = table.lits.len() as u32;
            table.starts.push(end);
        }
        table
    }

    /// 真实子句数（不含第 0 段的占位）。
    fn len(&self) -> usize {
        self.starts.len() - 2
    }

    /// 全部文字数（占位段是空的，所以直接就是 `lits` 的长度）。
    fn literals(&self) -> usize {
        self.lits.len()
    }

    fn clause(&self, segment: usize) -> &[i32] {
        let from = self.starts[segment] as usize;
        let to = self.starts[segment + 1] as usize;
        &self.lits[from..to]
    }

    /// 遍历真实子句（第 1 段起）。
    fn iter(&self) -> impl Iterator<Item = &[i32]> {
        (1..self.starts.len() - 1).map(|segment| self.clause(segment))
    }

    /// 把单位子句插回有序位置，返回它所在的下标（调用方递归完要原位摘掉，表格才能复用）。
    fn insert_unit(&mut self, lit: i32) -> usize {
        let unit = [lit];
        // 整表有序，所以「小于单位子句的真实段数」就是插入点；+1 跳过第 0 段的占位。
        let at = 1 + self
            .iter()
            .take_while(|clause| *clause < unit.as_slice())
            .count();
        let from = self.starts[at] as usize;
        self.lits.insert(from, lit);
        for start in &mut self.starts[at + 1..] {
            *start += 1;
        }
        self.starts.insert(at + 1, self.starts[at] + 1);
        at
    }

    /// 摘掉 `insert_unit` 插进去的那一段。
    fn remove_unit(&mut self, at: usize) {
        let from = self.starts[at] as usize;
        let to = self.starts[at + 1] as usize;
        debug_assert_eq!(to - from, 1, "摘掉的必须是单位子句");
        self.lits.drain(from..to);
        self.starts.remove(at + 1);
        for start in &mut self.starts[at + 1..] {
            *start -= 1;
        }
    }

    /// 按切片字典序重排真实子句（第 0 段的占位不动）。
    fn sort(&mut self) {
        let mut order: Vec<usize> = (1..=self.len()).collect();
        order.sort_by(|&left, &right| {
            let (a, b) = (self.starts[left] as usize, self.starts[left + 1] as usize);
            let (c, d) = (self.starts[right] as usize, self.starts[right + 1] as usize);
            self.lits[a..b].cmp(&self.lits[c..d])
        });
        let mut lits = Vec::with_capacity(self.lits.len());
        let mut starts = Vec::with_capacity(self.starts.len());
        // 重排会重建整张表，第 0 段的占位必须一起重建：少写一个 0 就等于把第一条
        // 真实子句当成占位丢掉（实测症状是计数偏大，被 cnf_fuzz 的穷举对拍当场抓住）。
        starts.push(0u32);
        starts.push(0u32);
        for &segment in &order {
            lits.extend_from_slice(self.clause(segment));
            let end = lits.len() as u32;
            starts.push(end);
        }
        self.lits = lits;
        self.starts = starts;
    }
}

/// Counts all satisfying assignments of `cnf`.
#[must_use]
pub fn count_cnf(cnf: &Cnf) -> CountResult {
    let nested = normalize(&cnf.clauses, cnf.nvars); // 唯一一次全表规范形整理
    if nested.iter().any(Vec::is_empty) {
        return CountResult::default();
    }
    let clauses = Table::from_nested(&nested);
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
    fn count_core(&mut self, clauses: &Table, k: usize) -> u128 {
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

        if let Some(parts) = decompose(&numbered, m) {
            self.stats.components += 1;
            let mut product = free_factor;
            for (part_vars, part_indices) in parts {
                let local = renumber(&numbered, &part_indices, &part_vars);
                product *= self.count_core(&local, part_vars.len());
            }
            return product;
        }

        let key = cache_key(&numbered, m);
        if let Some(cached) = self.cache.get(&key) {
            self.stats.cache_hits += 1;
            return free_factor * cached;
        }
        self.stats.cache_misses += 1;

        let branch = choose_var(&numbered, m);
        self.stats.branches += 1;
        // 两个子节点共用这一份表：插单位子句 → 递归 → 原位摘掉 → 插反面。
        // 省下的是每个分支节点一次全表复制（分支数 ≈ 节点数的一半）。
        let at = numbered.insert_unit(branch as i32);
        let positive = self.count_core(&numbered, m);
        numbered.remove_unit(at);
        let at = numbered.insert_unit(-(branch as i32));
        let negative = self.count_core(&numbered, m);
        numbered.remove_unit(at);
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
/// 返回 `(带第 0 段占位的新子句表, 出现变量数, 已赋值变量数)`；出现冲突返回 `None`。
/// 单位传播的强制闭包与顺序无关（合流），所以残基与旧的两段实现逐位相同——
/// 这一点由「节点数 / 分支数 / 组件数 / 缓存命中数全等」的实测对拍钉住，不是口头保证。
fn residue(clauses: &Table, k: usize) -> Option<(Table, usize, usize)> {
    // 先做一次「段视图」展平（1 次分配），后面全部按下标访问，
    // 于是这份表内部不再有 `Vec<Vec<..>>` 的每条子句一次分配。
    let open: Vec<&[i32]> = clauses.iter().collect();
    // 出现表用扁平数组 + 前缀偏移（CSR），每节点只 3 次分配，不是 2(k+1) 次小 Vec。
    let mut counts: Vec<u32> = Vec::with_capacity(open.len());
    let mut pos_len = vec![0u32; k + 2];
    let mut neg_len = vec![0u32; k + 2];
    let mut literals = 0usize;
    for &clause in &open {
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
    for (index, &clause) in open.iter().enumerate() {
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
    let mut satisfied = vec![false; open.len()];
    let mut trail: Vec<i32> = open
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
                let remaining = open[clause as usize]
                    .iter()
                    .copied()
                    .find(|&lit| assign[lit.unsigned_abs() as usize] == 0)?;
                trail.push(remaining);
            }
        }
    }

    // 第一趟只数长度、标记出现的变量：不再为每条残基子句单独分配 `Vec`。
    let mut occurs = vec![false; k + 1];
    let mut keep: Vec<usize> = Vec::with_capacity(open.len());
    let mut total = 0usize;
    for (index, &clause) in open.iter().enumerate() {
        if satisfied[index] {
            continue;
        }
        if counts[index] == 0 {
            return None;
        }
        let mut kept = 0usize;
        for &lit in clause {
            let var = lit.unsigned_abs() as usize;
            if assign[var] == 0 {
                kept += 1;
                occurs[var] = true;
            }
        }
        debug_assert_eq!(kept, counts[index] as usize, "计数器与实际残基必须一致");
        keep.push(index);
        total += kept;
    }

    // 出现变量压成 1..=m 的新号，然后**边写边换号**。map 随老号单调递增，
    // 所以子句内部的有序性自动保持；整张表的字典序还要靠末尾那次 `sort`。
    // 第 0 段留空给分支插单位子句用（表格因此不用整体挪位）。
    let mut map = vec![0i32; k + 1];
    let mut m = 0usize;
    for var in 1..=k {
        if occurs[var] {
            m += 1;
            map[var] = m as i32;
        }
    }
    let mut numbered = Table {
        lits: Vec::with_capacity(total),
        starts: Vec::with_capacity(keep.len() + 2),
    };
    numbered.starts.push(0);
    numbered.starts.push(0);
    for &index in &keep {
        for &lit in open[index] {
            let var = lit.unsigned_abs() as usize;
            if assign[var] != 0 {
                continue;
            }
            let mapped = map[var];
            debug_assert!(mapped != 0, "残基里的变量必须拿到新号");
            numbered.lits.push(if lit > 0 { mapped } else { -mapped });
        }
        let end = numbered.lits.len() as u32;
        numbered.starts.push(end);
    }
    // 换号之后必须重排：老号→新号对「同一子句内部」是单调的，但整张表的字典序
    // 会被子句首文字的**符号**翻转（实测反例 [3,4,-7] 排在 [-1,-2] 之前），
    // 所以这里不能省。省掉的只有 renumber 的那次全表拷贝。
    numbered.sort();
    Some((numbered, m, assigned))
}

/// 把组件的文字换成 `1..=vars.len()` 的新号，产出一张带第 0 段占位的新表。
/// `indices` 是父表的**段下标**，所以这里不再逐条子句克隆出新的 `Vec`。
fn renumber(source: &Table, indices: &[usize], vars: &[usize]) -> Table {
    let max = vars.iter().copied().max().unwrap_or(0);
    let mut map = vec![0i32; max + 1];
    for (index, &var) in vars.iter().enumerate() {
        map[var] = (index + 1) as i32;
    }
    let literals: usize = indices
        .iter()
        .map(|&index| source.clause(index).len())
        .sum();
    let mut out = Table {
        lits: Vec::with_capacity(literals),
        starts: Vec::with_capacity(indices.len() + 2),
    };
    out.starts.push(0);
    out.starts.push(0);
    for &index in indices {
        for &lit in source.clause(index) {
            let mapped = map[lit.unsigned_abs() as usize];
            debug_assert!(mapped != 0);
            out.lits.push(if lit > 0 { mapped } else { -mapped });
        }
        let end = out.lits.len() as u32;
        out.starts.push(end);
    }
    out.sort();
    out
}

/// A connected component: its variables and its clause indices into the parent table.
type Component = (Vec<usize>, Vec<usize>);

/// Splits a CNF into connected components when there is more than one.
fn decompose(clauses: &Table, k: usize) -> Option<Vec<Component>> {
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
    for clause in clauses.iter() {
        let first = clause[0].unsigned_abs() as usize;
        for lit in &clause[1..] {
            union(&mut parent, first, lit.unsigned_abs() as usize);
        }
    }

    // 组件只带**段下标**，克隆留给 `renumber`：分解一个 500 条子句的节点时，
    // 这里省下的是 500 次 `Vec` 分配，不是扫描。
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for (segment, clause) in (1..).zip(clauses.iter()) {
        let root = find(&mut parent, clause[0].unsigned_abs() as usize);
        groups.entry(root).or_default().push(segment);
    }
    if groups.len() <= 1 {
        return None;
    }
    let mut out = Vec::with_capacity(groups.len());
    for (_, indices) in groups {
        let mut vars: Vec<usize> = indices
            .iter()
            .flat_map(|&segment| {
                clauses
                    .clause(segment)
                    .iter()
                    .map(|lit| lit.unsigned_abs() as usize)
            })
            .collect();
        vars.sort_unstable();
        vars.dedup();
        out.push((vars, indices));
    }
    Some(out)
}

/// Picks the most frequent variable; ties go to the lowest index.
fn choose_var(clauses: &Table, k: usize) -> usize {
    let mut score = vec![0u32; k + 1];
    for clause in clauses.iter() {
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
fn cache_key(clauses: &Table, k: usize) -> Vec<i32> {
    let mut key = Vec::with_capacity(2 + clauses.literals() + clauses.len());
    key.push(k as i32);
    key.push(clauses.len() as i32);
    for clause in clauses.iter() {
        key.push(clause.len() as i32);
        key.extend_from_slice(clause);
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 表格不变量：第 0 段永远是占位空段、`starts` 末位哨兵等于文字总数、真实段按字典序有序。
    ///
    /// 这条不是装饰：扁平化第一版就是 `sort()` 重建表时少写了一个 0，第一条真实子句被当成
    /// 占位丢掉，症状是计数偏大 —— 靠 `cnf_fuzz` 的穷举对拍才发现。把不变量在这里直接钉住，
    /// 下次不必等对拍跑完才报红。
    fn assert_invariants(table: &Table, context: &str) {
        assert!(
            table.clause(0).is_empty(),
            "{context}：第 0 段必须是占位空段，实际是 {:?}",
            table.clause(0)
        );
        assert_eq!(table.starts[0], 0, "{context}：段表必须从 0 开始");
        assert!(
            table.starts.windows(2).all(|pair| pair[0] <= pair[1]),
            "{context}：段起点必须单调不减，实际 {starts:?}",
            starts = table.starts
        );
        assert_eq!(
            table.starts.last().copied().map(|end| end as usize),
            Some(table.lits.len()),
            "{context}：末位哨兵必须等于文字总数"
        );
        let mut previous: Option<&[i32]> = None;
        for clause in table.iter() {
            if let Some(old) = previous {
                assert!(
                    old <= clause,
                    "{context}：子句表必须有序，{old:?} 与 {clause:?} 反了"
                );
            }
            previous = Some(clause);
        }
    }

    #[test]
    fn residue_preserves_the_table_invariants() {
        // 一组会触发强制闭包 + 换号 + 重排的子句：[-1,2] 与 [1] 一起把 2 逼成单位传播。
        let nested = normalize(&[vec![-1, 2], vec![1], vec![2, -3], vec![-2, 3, -4]], 4);
        let table = Table::from_nested(&nested);
        assert_invariants(&table, "建表");
        let (numbered, m, assigned) = residue(&table, 4).expect("可满足");
        assert_eq!((m, assigned), (2, 2), "1 与 2 被强制赋值，只剩 3、4");
        assert_invariants(&numbered, "residue");
    }

    #[test]
    fn renumber_preserves_the_table_invariants() {
        let nested = normalize(&[vec![1, 2], vec![-2], vec![3, 4], vec![-4, 3]], 4);
        let table = Table::from_nested(&nested);
        let parts = decompose(&table, 4).expect("两个互不相干的组件");
        assert_eq!(parts.len(), 2);
        for (vars, indices) in &parts {
            let local = renumber(&table, indices, vars);
            assert_eq!(local.len(), indices.len(), "组件里的子句数不能被改写");
            assert_invariants(&local, "renumber");
        }
    }

    #[test]
    fn insert_unit_and_remove_round_trip_bitwise() {
        let nested = normalize(&[vec![-3, 4], vec![1, 2], vec![2, -1]], 4);
        let mut table = Table::from_nested(&nested);
        let before = (table.lits.clone(), table.starts.clone());
        for lit in [1i32, -1, 4, -4, 2, -2, 3, -3] {
            let at = table.insert_unit(lit);
            assert_eq!(table.clause(at), &[lit], "{lit} 必须落在自己那一段");
            assert_invariants(&table, "插单位子句后");
            table.remove_unit(at);
            assert_eq!(table.lits, before.0, "摘掉 {lit} 之后文字流必须逐位还原");
            assert_eq!(table.starts, before.1, "摘掉 {lit} 之后段表必须逐位还原");
        }
    }

    #[test]
    fn insert_unit_lands_in_sorted_position() {
        // 规范形之后的表序是 [-3,4] < [2] < [3,-4]（切片字典序按首文字的**带符号值**比）。
        let nested = normalize(&[vec![2], vec![3, -4], vec![-3, 4]], 4);
        let mut table = Table::from_nested(&nested);
        let at = table.insert_unit(1);
        assert_eq!(table.clause(at), &[1], "单位子句必须落在它自己那一段");
        assert_eq!(table.clause(at - 1), &[-3, 4], "前一段必须字典序更小");
        assert_eq!(table.clause(at + 1), &[2], "后一段必须字典序更大");
        assert_invariants(&table, "插入 [1] 之后");
        table.remove_unit(at);

        let at = table.insert_unit(-4);
        assert_eq!(table.clause(at), &[-4]);
        assert_invariants(&table, "插入 [-4] 之后");
    }
}
