//! 三件套（预测 / 学习 / 进化）攻 NP 完全问题的实例分布：匹配族模型计数。
//!
//! **诚实口径**：这是变量序搜索的**实例分布工程实验**——DPLL 的最坏复杂度
//! 不变，P vs NP 仍然开放；不存在"接近 P=NP"的结果。读数回答的是：在
//! BDD 教科书级坏变量序的匹配族上，进化 + 学习 + 预测能把节点数/时间压
//! 多少，而且必须与**同评估预算的随机对照**比。
//!
//! 注入点（不碰 DPLL 内部）：
//! 1. **进化**：变量下标置换基因组（纯语法双射，保计数），重排 CNF 文字；
//!    `residue` 每节点按老号升序重编号、`choose_var` 以 `Reverse(var)` 决胜，
//!    所以置换会改变整棵搜索树。
//! 2. **预测**：EWMA 跟踪每代最优节点数，停滞 → 增大变异跨度。
//! 3. **学习**：QAT 三值分类器（`trit-learn`）从已评估置换的特征预测
//!    「当前集合中位数以上」的质量，用于下一代候选预过滤；样本太单薄或
//!    训练集准确率 < 0.60 时明确旁路，不假装它在工作。
//!
//! 四种模式（`--mode`）：
//! - `random`：同评估预算的随机置换对照（消融下限）；
//! - `evo`：进化（精英保留 + OX 交叉 + 固定跨度变异）；
//! - `evo-adapt`：进化 + EWMA 自适应变异跨度；
//! - `full`：进化 + 自适应 + 学习过滤。
//!
//! 实例族（`--family`）：
//! - `matching`：闭式 `4^n - 3^n`；但结构全对称，实测变量下标置换对 DPLL
//!   搜索树零影响（82 个置换全部 683 节点）——留作「置换无效」的对照。
//! - `random3sat`：随机 3-SAT（默认 24 变量 / 102 子句，相变比附近）；无闭式，
//!   正确性靠「每个置换的计数与基线逐位一致」这条不变性门。
//!
//! 正确性门：每个置换的计数必须与基线逐位一致；恒等置换必须复现基线节点数；
//! 基线本身对 `4^pairs - 3^pairs` 闭式（与 `tests/crosscheck.rs` 同一口径）。
//!
//! ```bash
//! cargo run --release -p trit-count --example evolved_count -- --pairs 24 --mode full
//! ```

use std::collections::{BTreeMap, HashSet};
use std::time::Instant;

use serde_json::json;
use trit_core::algebra::Algebra;
use trit_core::ast::Expr;
use trit_core::parser::Parser;
use trit_core::value::Ternary;
use trit_count::cnf;
use trit_count::dpll;
use trit_learn::dense::TernaryDense;
use trit_learn::loss::cross_entropy;
use trit_learn::opt::Adam;
use trit_learn::{Activation, Ste};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Random,
    Evo,
    EvoAdapt,
    Full,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    /// 匹配族 `(x0&y0)|…`，闭式 `4^n - 3^n`；变量下标置换的零方差对照。
    Matching,
    /// 直接构造的随机 3-SAT（相变比 4.26 附近）；无闭式，正确性靠置换不变性。
    Random3Sat,
}
impl Family {
    fn parse(text: &str) -> Self {
        match text {
            "matching" => Self::Matching,
            "random3sat" => Self::Random3Sat,
            other => panic!("未知 --family {other}（matching / random3sat）"),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Matching => "matching",
            Self::Random3Sat => "random3sat",
        }
    }
}
impl Mode {
    fn parse(text: &str) -> Self {
        match text {
            "random" => Self::Random,
            "evo" => Self::Evo,
            "evo-adapt" => Self::EvoAdapt,
            "full" => Self::Full,
            other => panic!("未知 --mode {other}（random / evo / evo-adapt / full）"),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Random => "random",
            Self::Evo => "evo",
            Self::EvoAdapt => "evo-adapt",
            Self::Full => "full",
        }
    }
}

/// xorshift64*。
struct Rng(u64);

impl Rng {
    /// SplitMix64 混合后再置奇数：直接的 `seed | 1` 会把 6/7、8/9 塌成同一条流，
    /// 让「不同实例种子」伪装成独立样本（本实验第一版就踩了：20 个实例只剩 11 个不同 CNF）。
    fn new(seed: u64) -> Self {
        let mut z = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        let z = z ^ (z >> 31);
        Self(z | 1)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn next_f32(&mut self) -> f32 {
        (self.next_u64() % 2001) as f32 / 1000.0 - 1.0
    }
    fn next_index(&mut self, bound: usize) -> usize {
        usize::try_from(self.next_u64() % bound as u64).expect("bound > 0")
    }
    fn shuffle<T>(&mut self, items: &mut [T]) {
        for index in (1..items.len()).rev() {
            let other = self.next_index(index + 1);
            items.swap(index, other);
        }
    }
}

fn matching_family(pairs: usize) -> (Expr, Vec<String>, HashSet<String>) {
    let xs: Vec<String> = (0..pairs).map(|i| format!("x{i}")).collect();
    let ys: Vec<String> = (0..pairs).map(|i| format!("y{i}")).collect();
    let mut source = String::new();
    for (x, y) in xs.iter().zip(&ys) {
        let term = format!("({x} & {y})");
        source = if source.is_empty() {
            term
        } else {
            format!("({source} | {term})")
        };
    }
    let declared: Vec<String> = xs.iter().chain(&ys).cloned().collect();
    let binary: HashSet<String> = declared.iter().cloned().collect();
    (
        Parser::parse(&source).expect("匹配族解析"),
        declared,
        binary,
    )
}

/// 随机 3-SAT：子句内变量互异、子句不重复、正负号随机；变量 1..=vars。
fn random_3sat(vars: usize, clauses: usize, rng: &mut Rng) -> cnf::Cnf {
    assert!(vars >= 3, "3-SAT 至少需要 3 个变量");
    let mut seen = HashSet::new();
    let mut out: Vec<Vec<i32>> = Vec::with_capacity(clauses);
    let mut guard = 0usize;
    while out.len() < clauses {
        guard += 1;
        assert!(guard < clauses * 1000, "子句空间太小，生成器卡死");
        let mut picked: Vec<usize> = Vec::with_capacity(3);
        while picked.len() < 3 {
            let var = 1 + rng.next_index(vars);
            if !picked.contains(&var) {
                picked.push(var);
            }
        }
        let mut key = picked.clone();
        key.sort_unstable();
        if !seen.insert(key) {
            continue;
        }
        out.push(
            picked
                .iter()
                .map(|&var| {
                    if rng.next_index(2) == 0 {
                        -(var as i32)
                    } else {
                        var as i32
                    }
                })
                .collect(),
        );
    }
    cnf::Cnf {
        nvars: vars,
        clauses: out,
    }
}

/// 对 CNF 做变量下标双射重映射（纯语法置换，保计数——满足赋值双射对应）。
///
/// `indices` 是允许置换的下标集合（升序）；`perm[i]` 是放到 `indices[i]`
/// 位置的**旧下标**，必须是 `indices` 的一个排列。其余下标（如匹配族编码的
/// `false_var = 1`）固定不动。
fn permute_cnf(base: &cnf::Cnf, indices: &[usize], perm: &[usize]) -> cnf::Cnf {
    assert_eq!(perm.len(), indices.len(), "置换长度必须等于可置换下标数");
    let mut remap: Vec<usize> = (0..=base.nvars).collect();
    let mut seen = vec![false; base.nvars + 1];
    for (slot, &old) in perm.iter().enumerate() {
        assert!(indices.contains(&old), "旧下标不在可置换集合里：{old}");
        assert!(!seen[old], "置换里有重复下标：{old}");
        seen[old] = true;
        remap[old] = indices[slot];
    }
    for &index in indices {
        assert!(remap[index] <= base.nvars, "映射越界：{index}");
    }
    let remap_lit = |lit: i32| -> i32 {
        let mapped = remap[lit.unsigned_abs() as usize];
        assert!(mapped != 0, "文字里出现了未映射变量：{lit}");
        if lit < 0 {
            -(mapped as i32)
        } else {
            mapped as i32
        }
    };
    cnf::Cnf {
        nvars: base.nvars,
        clauses: base
            .clauses
            .iter()
            .map(|clause| clause.iter().map(|&lit| remap_lit(lit)).collect())
            .collect(),
    }
}

/// 置换的 cheap 特征（学习组件的输入）；`offset` 是可置换下标的下界。
fn perm_features(perm: &[usize], n: usize, offset: usize) -> Vec<f32> {
    let norm = n.max(1) as f32;
    let mut moved = 0usize;
    let mut displacement = 0.0f32;
    let mut inversions = 0usize;
    for (slot, &old) in perm.iter().enumerate() {
        let original = old - offset;
        if original != slot {
            moved += 1;
        }
        displacement += (original as i64 - slot as i64).unsigned_abs() as f32 / norm;
    }
    for left in 0..perm.len() {
        for right in left + 1..perm.len() {
            if perm[left] > perm[right] {
                inversions += 1;
            }
        }
    }
    let pairs = (n * n.saturating_sub(1) / 2).max(1) as f32;
    vec![
        moved as f32 / norm,
        displacement / norm,
        inversions as f32 / pairs,
        (perm.first().copied().unwrap_or(2) - 2) as f32 / norm,
        (perm.last().copied().unwrap_or(2) - 2) as f32 / norm,
        (perm.get(n / 2).copied().unwrap_or(2) - 2) as f32 / norm,
    ]
}

fn argmax(logits: &[f32]) -> usize {
    logits
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).expect("有限 logits"))
        .map(|(class, _)| class)
        .expect("非空 logits")
}

fn random_perm(indices: &[usize], rng: &mut Rng) -> Vec<usize> {
    let mut perm = indices.to_vec();
    rng.shuffle(&mut perm);
    perm
}

/// 顺序交叉（OX1）：保留 `a` 的一段，其余按 `b` 的顺序填充，子代仍是合法置换。
fn order_crossover(a: &[usize], b: &[usize], cut1: usize, cut2: usize) -> Vec<usize> {
    let n = a.len();
    let mut child = vec![0usize; n];
    let mut taken = vec![false; n + 2];
    for slot in cut1..cut2 {
        child[slot] = a[slot];
        taken[a[slot]] = true;
    }
    let mut slot = cut2 % n;
    for step in 0..n {
        let value = b[(cut2 + step) % n];
        if !taken[value] {
            while child[slot] != 0 {
                slot = (slot + 1) % n;
            }
            child[slot] = value;
            taken[value] = true;
            slot = (slot + 1) % n;
        }
    }
    child
}

fn mutate(perm: &mut [usize], span: usize, rng: &mut Rng) {
    let n = perm.len();
    for _ in 0..span.max(1) {
        let left = rng.next_index(n);
        let right = rng.next_index(n);
        perm.swap(left, right);
    }
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).expect("有限节点数"));
    sorted[sorted.len() / 2]
}

fn make_layer(rows: usize, cols: usize, rng: &mut Rng) -> TernaryDense {
    let latent: Vec<Vec<f32>> = (0..rows)
        .map(|_| (0..cols).map(|_| rng.next_f32() * 0.05).collect())
        .collect();
    let flat: Vec<f32> = latent.iter().flatten().copied().collect();
    let scale0 = |row: usize| {
        let mean = latent[row].iter().map(|w| w.abs()).sum::<f32>() / cols as f32;
        if mean > 1e-6 { mean } else { 1.0 }
    };
    TernaryDense::new(
        rows,
        cols,
        |row, col| flat[row * cols + col],
        scale0,
        Ste::Clipped { ratio: 1.0 },
    )
}

/// 从已评估的化石训练「中位数以上」分类器；类别太单薄、或训练集准确率
/// 低于 0.60（QAT 三值层拟合不动）就返回 `None`（旁路，不假装它在工作）。
fn train_classifier(
    training: &[(Vec<f32>, usize)],
    dim: usize,
    rng: &mut Rng,
) -> Option<(TernaryDense, TernaryDense, f32)> {
    let positives = training.iter().filter(|(_, label)| *label == 1).count();
    let negatives = training.len() - positives;
    if training.len() < 8 || positives < 3 || negatives < 3 {
        return None;
    }
    let mut guard = make_layer(32, dim, rng);
    let mut out = make_layer(2, 32, rng);
    let mut adam_gl = Adam::new(1e-2);
    let mut adam_gs = Adam::new(1e-2);
    let mut adam_ol = Adam::new(1e-2);
    let mut adam_os = Adam::new(1e-2);
    for _ in 0..80 {
        for (feats, label) in training {
            guard.zero_grad();
            out.zero_grad();
            let hidden = guard.forward(feats);
            let activated: Vec<f32> = hidden
                .iter()
                .map(|&v| Activation::Tanh.forward(v))
                .collect();
            let logits = out.forward(&activated);
            let (_loss, grad_logits) = cross_entropy(&logits, *label);
            let grad_activated = out.backward(&grad_logits);
            out.update(&mut adam_ol, &mut adam_os);
            let grad_hidden: Vec<f32> = grad_activated
                .iter()
                .zip(&activated)
                .map(|(&grad, &value)| grad * Activation::Tanh.backward(value))
                .collect();
            let _ = guard.backward(&grad_hidden);
            guard.update(&mut adam_gl, &mut adam_gs);
        }
    }
    let mut correct = 0usize;
    for (feats, label) in training {
        let hidden = guard.forward(feats);
        let activated: Vec<f32> = hidden
            .iter()
            .map(|&v| Activation::Tanh.forward(v))
            .collect();
        if argmax(&out.forward(&activated)) == *label {
            correct += 1;
        }
    }
    let accuracy = correct as f32 / training.len() as f32;
    if accuracy < 0.6 {
        return None;
    }
    Some((guard, out, accuracy))
}

fn classifier_accepts(guard: &mut TernaryDense, out: &mut TernaryDense, feats: &[f32]) -> bool {
    let hidden = guard.forward(feats);
    let activated: Vec<f32> = hidden
        .iter()
        .map(|&v| Activation::Tanh.forward(v))
        .collect();
    argmax(&out.forward(&activated)) == 1
}

struct Evaluator<'a> {
    base: &'a cnf::Cnf,
    indices: Vec<usize>,
    baseline_count: u128,
    fossils: BTreeMap<Vec<usize>, f64>,
}

impl Evaluator<'_> {
    /// 评估一个置换；每个结果都过计数不变性断言后才入化石。
    fn evaluate(&mut self, perm: &[usize]) -> f64 {
        if let Some(&nodes) = self.fossils.get(perm) {
            return nodes;
        }
        let permuted = permute_cnf(self.base, &self.indices, perm);
        let result = dpll::count_cnf(&permuted);
        assert_eq!(
            result.count, self.baseline_count,
            "置换后计数必须不变（变量双射重命名）"
        );
        let nodes = result.stats.nodes as f64;
        self.fossils.insert(perm.to_vec(), nodes);
        nodes
    }

    fn best(&self) -> (&Vec<usize>, f64) {
        self.fossils
            .iter()
            .min_by(|a, b| a.1.partial_cmp(b.1).expect("有限节点数"))
            .map(|(perm, &nodes)| (perm, nodes))
            .expect("至少评估过恒等置换")
    }
}

fn main() {
    let mut pairs = 24usize;
    let mut generations = 8usize;
    let mut pop = 10usize;
    let mut seed = 7u64;
    let mut instance_seed: Option<u64> = None;
    let mut mode = Mode::Full;
    let mut family_kind = Family::Matching;
    let mut vars = 24usize;
    let mut clauses = 102usize;
    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--pairs" => pairs = argv.next().expect("pairs").parse().expect("pairs"),
            "--generations" => {
                generations = argv
                    .next()
                    .expect("generations")
                    .parse()
                    .expect("generations");
            }
            "--pop" => pop = argv.next().expect("pop").parse().expect("pop"),
            "--seed" => seed = argv.next().expect("seed").parse().expect("seed"),
            "--instance-seed" => {
                instance_seed = Some(
                    argv.next()
                        .expect("instance-seed")
                        .parse()
                        .expect("instance-seed"),
                );
            }
            "--mode" => mode = Mode::parse(&argv.next().expect("mode")),
            "--family" => family_kind = Family::parse(&argv.next().expect("family")),
            "--vars" => vars = argv.next().expect("vars").parse().expect("vars"),
            "--clauses" => clauses = argv.next().expect("clauses").parse().expect("clauses"),
            other => panic!("未知参数 {other}"),
        }
    }
    assert!(pairs >= 2 && pop >= 2 && generations >= 1);
    let instance_seed = instance_seed.unwrap_or(seed ^ 0x5eed_5eed);

    let (base_cnf, closed) = match family_kind {
        Family::Matching => {
            let (expr, declared, binary) = matching_family(pairs);
            let cnf =
                cnf::Encoder::new(Algebra::Kleene, &declared, &binary).encode(&expr, Ternary::True);
            // 匹配族闭式：4^n - 3^n（每对 4 种布尔赋值，OR 为真的补集是每对 3 种非双真）。
            (cnf, Some(4u128.pow(pairs as u32) - 3u128.pow(pairs as u32)))
        }
        Family::Random3Sat => (
            random_3sat(vars, clauses, &mut Rng::new(instance_seed)),
            None,
        ),
    };
    let baseline_start = Instant::now();
    let baseline = dpll::count_cnf(&base_cnf);
    let baseline_ms = baseline_start.elapsed().as_secs_f64() * 1e3;
    if let Some(expected) = closed {
        assert_eq!(baseline.count, expected, "基线计数必须等于闭式");
    }
    let baseline_nodes = baseline.stats.nodes as f64;
    println!(
        "{}",
        json!({
            "stage": "baseline",
            "family": family_kind.name(),
            "mode": mode.name(),
            "seed": seed,
            "instance_seed": instance_seed,
            "pairs": pairs,
            "vars": base_cnf.nvars,
            "clauses": base_cnf.clauses.len(),
            "nodes": baseline.stats.nodes,
            "branches": baseline.stats.branches,
            "ms": baseline_ms,
            "closed_form": closed.map(|value| value.to_string()),
        })
    );

    let indices: Vec<usize> = match family_kind {
        Family::Matching => (2..=base_cnf.nvars).collect(),
        Family::Random3Sat => (1..=base_cnf.nvars).collect(),
    };
    let n = indices.len();
    let identity = indices.clone();
    let mut rng = Rng::new(seed);
    let mut evaluator = Evaluator {
        base: &base_cnf,
        indices: indices.clone(),
        baseline_count: baseline.count,
        fossils: BTreeMap::new(),
    };
    let identity_nodes = evaluator.evaluate(&identity);
    assert_eq!(
        identity_nodes, baseline_nodes,
        "恒等置换必须复现基线节点数（置换实现自检）"
    );

    let budget = pop * generations;
    let mut span = (n / 8).max(1);
    let mut ewma = baseline_nodes;
    let mut generation = 0usize;
    let mut accepted_total = 0usize;
    let mut rejected_total = 0usize;
    let mut forced_total = 0usize;

    if mode == Mode::Random {
        while evaluator.fossils.len() < budget && generation < generations * 4 {
            generation += 1;
            for _ in 0..pop {
                if evaluator.fossils.len() >= budget {
                    break;
                }
                let perm = random_perm(&indices, &mut rng);
                evaluator.evaluate(&perm);
            }
            let (_, best) = evaluator.best();
            println!(
                "{}",
                json!({
                    "stage": "generation",
                    "mode": mode.name(),
                    "seed": seed,
                    "generation": generation,
                    "evaluations": evaluator.fossils.len(),
                    "best_nodes": best,
                    "baseline_nodes": baseline_nodes,
                })
            );
        }
    } else {
        let mut population: Vec<Vec<usize>> = vec![identity.clone()];
        while population.len() < pop {
            population.push(random_perm(&indices, &mut rng));
        }
        while evaluator.fossils.len() < budget && generation < generations * 4 {
            let mut scored: Vec<(f64, usize)> = population
                .iter()
                .enumerate()
                .map(|(slot, perm)| (evaluator.evaluate(perm), slot))
                .collect();
            scored.sort_by(|a, b| a.0.partial_cmp(&b.0).expect("有限节点数"));
            let generation_best = scored[0].0;

            if mode != Mode::Evo {
                if generation == 0 {
                    ewma = generation_best;
                } else {
                    let previous = ewma;
                    ewma = 0.4 * generation_best + 0.6 * ewma;
                    let ceiling = (n / 4).max(2);
                    span = if ewma >= previous * 0.98 {
                        (span * 2).min(ceiling)
                    } else {
                        (span / 2).max(1)
                    };
                }
            }

            // 学习组件：标签 = 当前化石集合的中位数以上（相对质量代理）。
            let mut classifier = None;
            let mut accuracy = None;
            if mode == Mode::Full {
                let node_values: Vec<f64> = evaluator.fossils.values().copied().collect();
                let threshold = median(&node_values);
                let training: Vec<(Vec<f32>, usize)> = evaluator
                    .fossils
                    .iter()
                    .map(|(perm, &nodes)| {
                        (
                            perm_features(perm, n, indices[0]),
                            usize::from(nodes <= threshold),
                        )
                    })
                    .collect();
                if let Some((guard, out, acc)) = train_classifier(&training, 6, &mut rng) {
                    classifier = Some((guard, out));
                    accuracy = Some(acc);
                }
            }

            let mut next_population: Vec<Vec<usize>> = vec![
                population[scored[0].1].clone(),
                population[scored[1].1].clone(),
            ];
            let mut accepted = 0usize;
            let mut rejected = 0usize;
            let mut forced = 0usize;
            let mut attempts = 0usize;
            while next_population.len() < pop && attempts < pop * 8 {
                attempts += 1;
                let left = &population[scored[rng.next_index(scored.len())].1];
                let right = &population[scored[rng.next_index(scored.len())].1];
                let cut1 = rng.next_index(n);
                let cut2 = (cut1 + 1 + rng.next_index(n - cut1)).min(n);
                let mut child = order_crossover(left, right, cut1, cut2);
                mutate(&mut child, span, &mut rng);
                let keep = match classifier.as_mut() {
                    Some((guard, out)) => {
                        classifier_accepts(guard, out, &perm_features(&child, n, indices[0]))
                    }
                    None => true,
                };
                if keep {
                    accepted += 1;
                    next_population.push(child);
                } else if attempts >= pop * 8 {
                    forced += 1;
                    next_population.push(child);
                } else {
                    rejected += 1;
                }
            }
            while next_population.len() < pop {
                forced += 1;
                next_population.push(random_perm(&indices, &mut rng));
            }
            accepted_total += accepted;
            rejected_total += rejected;
            forced_total += forced;
            let (_, best_so_far) = evaluator.best();
            println!(
                "{}",
                json!({
                    "stage": "generation",
                    "mode": mode.name(),
                    "seed": seed,
                    "generation": generation,
                    "evaluations": evaluator.fossils.len(),
                    "generation_best": generation_best,
                    "best_nodes": best_so_far,
                    "baseline_nodes": baseline_nodes,
                    "ewma": ewma,
                    "mutation_span": span,
                    "learning_accepted": accepted,
                    "learning_rejected": rejected,
                    "learning_forced": forced,
                    "classifier_accuracy": accuracy,
                    "classifier_bypassed": mode == Mode::Full && accuracy.is_none(),
                })
            );
            population = next_population;
            generation += 1;
        }
    }

    let (best_perm, _) = evaluator.best();
    let best_perm = best_perm.clone();
    let best_cnf = permute_cnf(&base_cnf, &indices, &best_perm);
    let best_start = Instant::now();
    let best = dpll::count_cnf(&best_cnf);
    let best_ms = best_start.elapsed().as_secs_f64() * 1e3;
    assert_eq!(best.count, baseline.count, "最优置换计数必须等于基线");
    println!(
        "{}",
        json!({
            "stage": "final",
            "family": family_kind.name(),
            "mode": mode.name(),
            "seed": seed,
            "instance_seed": instance_seed,
            "pairs": pairs,
            "vars": base_cnf.nvars,
            "clauses": base_cnf.clauses.len(),
            "budget": budget,
            "evaluations": evaluator.fossils.len(),
            "baseline_nodes": baseline.stats.nodes,
            "best_nodes": best.stats.nodes,
            "node_ratio": baseline.stats.nodes as f64 / best.stats.nodes.max(1) as f64,
            "baseline_ms": baseline_ms,
            "best_ms": best_ms,
            "count_ok": best.count == baseline.count,
            "closed_form_ok": closed.map(|expected| best.count == expected),
            "learning_accepted_total": accepted_total,
            "learning_rejected_total": rejected_total,
            "learning_forced_total": forced_total,
        })
    );
}
