//! 随机划分（按句 id 分组）后的阈值对齐读数：重复多次，给区间而不是点估计。
//!
//! 之前 `editset_aligned` 按 pairs 索引切 valid/test，同一句可能同时出现在两侧（主题泄漏）。
//! 这里先把 pair 的 `a`/`b` 用向量 hash 聚成“句组”（并查集），再按组随机分到 valid/test，
//! 同一句不会跨两侧；重复 `--repeat` 次（不同 `--split-seed`）给出阈值与两个数字的区间。
//!
//! ```bash
//! cargo run --release -p trit-wspace --example editset_random_split -- \
//!   --pairs sts-bench.json --safetensors model.safetensors \
//!   --tuned tuned.safetensors --arch ffn --repeat 5 --split-seed 1
//! ```

use std::collections::BTreeMap;

use serde_json::{Value, json};
use trit_wspace::safetensors::{self, Weights};
use trit_wspace::{Edit, Net, Patch, Trit, delta};

const QUERY: &str = "encoder.layer.0.attention.self.query.weight";
const T1: &str = "encoder.layer.0.intermediate.dense.weight";
const T2: &str = "encoder.layer.0.output.dense.weight";

struct Args {
    pairs: String,
    safetensors: String,
    tuned: Option<String>,
    arch: String,
    valid_ratio: f32,
    repeat: usize,
    split_seed: u64,
    out: Option<String>,
}

fn parse_args() -> Args {
    let mut args = Args {
        pairs: String::new(),
        safetensors: String::new(),
        tuned: None,
        arch: "ffn".to_string(),
        valid_ratio: 0.3,
        repeat: 5,
        split_seed: 1,
        out: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut next = || it.next().expect("缺少参数值");
        match arg.as_str() {
            "--pairs" => args.pairs = next(),
            "--safetensors" => args.safetensors = next(),
            "--tuned" => args.tuned = Some(next()),
            "--arch" => args.arch = next(),
            "--valid-ratio" => args.valid_ratio = next().parse().expect("valid-ratio"),
            "--repeat" => args.repeat = next().parse().expect("repeat"),
            "--split-seed" => args.split_seed = next().parse().expect("split-seed"),
            "--out" => args.out = Some(next()),
            other => panic!("未知参数 {other}"),
        }
    }
    assert!(!args.pairs.is_empty() && !args.safetensors.is_empty());
    assert!(args.arch == "single" || args.arch == "ffn");
    assert!(args.repeat > 0);
    args
}

fn trit_row(values: &[f32]) -> Vec<Trit> {
    let weights = Weights {
        rows: 1,
        cols: values.len(),
        values: values.to_vec(),
    };
    let layer = weights.to_layer().expect("ternarize row");
    (0..values.len())
        .map(|col| layer.trit(0, col).expect("trit"))
        .collect()
}

fn cos_f32(left: &[f32], right: &[f32]) -> f32 {
    let dot: f32 = left.iter().zip(right).map(|(a, b)| a * b).sum();
    let norm = left.iter().map(|v| v * v).sum::<f32>().sqrt()
        * right.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm == 0.0 { 0.0 } else { dot / norm }
}

fn cos_trit(left: &[Trit], right: &[Trit]) -> f32 {
    let a: Vec<f32> = left.iter().map(|t| f32::from(t.to_i8())).collect();
    let b: Vec<f32> = right.iter().map(|t| f32::from(t.to_i8())).collect();
    cos_f32(&a, &b)
}

fn floats(item: &Value, key: &str) -> Vec<f32> {
    let values = item
        .get(key)
        .and_then(Value::as_array)
        .expect("pairs.a/b 必须是数组");
    values
        .iter()
        .map(|value| value.as_f64().expect("浮点数") as f32)
        .collect()
}

/// 向量内容的 64 位指纹（FNV-1a），作句 id。
fn sentence_id(values: &[f32]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for value in values {
        for byte in value.to_bits().to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
    }
    hash
}

struct Groups {
    ids: Vec<(u64, u64)>,
    parent: BTreeMap<u64, u64>,
}

impl Groups {
    fn new(pairs: &[(u64, u64)]) -> Self {
        let mut parent = BTreeMap::new();
        for &(a, b) in pairs {
            parent.entry(a).or_insert(a);
            parent.entry(b).or_insert(b);
        }
        let mut groups = Self {
            ids: pairs.to_vec(),
            parent,
        };
        for &(a, b) in pairs {
            groups.union(a, b);
        }
        groups
    }

    fn find(&mut self, x: u64) -> u64 {
        let parent = *self.parent.get(&x).expect("id");
        if parent == x {
            return x;
        }
        let root = self.find(parent);
        self.parent.insert(x, root);
        root
    }

    fn union(&mut self, a: u64, b: u64) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent.insert(ra, rb);
        }
    }

    /// 按组随机划分：返回 (valid pair 索引, test pair 索引)。
    fn split(&mut self, ratio: f32, seed: u64) -> (Vec<usize>, Vec<usize>) {
        let mut state = seed ^ 0x9E37_79B9_7F4A_7C15;
        if state == 0 {
            state = 1;
        }
        let mut next = || {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            state.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };
        // 每个 pair 先算根，再给根打分：分数落在前 ratio 的组进 valid。
        let roots: Vec<u64> = (0..self.ids.len())
            .map(|index| {
                let (a, b) = self.ids[index];
                let (ra, rb) = (self.find(a), self.find(b));
                if ra == rb { ra } else { rb }
            })
            .collect();
        let mut group_scores: BTreeMap<u64, u64> = BTreeMap::new();
        for root in &roots {
            group_scores.entry(*root).or_insert_with(&mut next);
        }
        let mut valid = Vec::new();
        let mut test = Vec::new();
        for (index, root) in roots.iter().enumerate() {
            let score = *group_scores.get(root).expect("score");
            if (score % 1000) < (ratio * 1000.0) as u64 {
                valid.push(index);
            } else {
                test.push(index);
            }
        }
        (valid, test)
    }
}

fn base_net(args: &Args) -> Net {
    if args.arch == "single" {
        let w = safetensors::read_tensor(&args.safetensors, QUERY).expect("query");
        Net::new(vec![w.to_layer().expect("ternary query")]).expect("single net")
    } else {
        let w1 = safetensors::read_tensor(&args.safetensors, T1).expect("intermediate");
        let w2 = safetensors::read_tensor(&args.safetensors, T2).expect("output");
        Net::new(vec![
            w1.to_layer().expect("ternary intermediate"),
            w2.to_layer().expect("ternary output"),
        ])
        .expect("two-layer net")
    }
}

fn shift(edit: &Edit) -> Edit {
    match edit {
        Edit::SetTrit {
            layer,
            row,
            col,
            to,
        } => Edit::SetTrit {
            layer: layer + 1,
            row: *row,
            col: *col,
            to: *to,
        },
        Edit::SetBias { layer, row, to } => Edit::SetBias {
            layer: layer + 1,
            row: *row,
            to: *to,
        },
        Edit::SetScale { layer, row, to } => Edit::SetScale {
            layer: layer + 1,
            row: *row,
            to: *to,
        },
        other => (*other).clone(),
    }
}

fn apply_tuned(args: &Args) -> (Net, usize) {
    let path = args.tuned.as_ref().expect("tuned");
    if args.arch == "single" {
        let base = safetensors::read_tensor(&args.safetensors, QUERY).expect("query");
        let tuned = safetensors::read_tensor(path, QUERY).expect("tuned query");
        let (patch, base_net, stats) =
            delta::diff_weights(&base, &tuned, None, None, 1.0).expect("diff");
        let mut edited = base_net.clone();
        patch.apply(&mut edited).expect("apply");
        (edited, stats.changed())
    } else {
        let base1 = safetensors::read_tensor(&args.safetensors, T1).expect("intermediate");
        let base2 = safetensors::read_tensor(&args.safetensors, T2).expect("output");
        let t1 = safetensors::read_tensor(path, T1).expect("tuned intermediate");
        let t2 = safetensors::read_tensor(path, T2).expect("tuned output");
        let (patch1, base_net1, stats1) =
            delta::diff_weights(&base1, &t1, None, None, 1.0).expect("diff 0");
        let (patch2, base_net2, stats2) =
            delta::diff_weights(&base2, &t2, None, None, 1.0).expect("diff 1");
        let mut edits: Vec<Edit> = patch1.edits().to_vec();
        edits.extend(patch2.edits().iter().map(shift));
        let mut edited = Net::new(vec![
            base_net1.layers()[0].clone(),
            base_net2.layers()[0].clone(),
        ])
        .expect("base net");
        Patch::new(edits).apply(&mut edited).expect("apply");
        (edited, stats1.changed() + stats2.changed())
    }
}

fn scores(net: &Net, pairs: &[(Vec<Trit>, Vec<Trit>, bool)]) -> Vec<f32> {
    pairs
        .iter()
        .map(|(left, right, _)| {
            let a = net.forward(left).expect("forward a");
            cos_trit(&a, &net.forward(right).expect("forward b"))
        })
        .collect()
}

fn best_threshold(want: &[bool], scores: &[f32], index: &[usize]) -> (f32, f32) {
    let grid: Vec<f32> = (1..=95).map(|step| step as f32 / 100.0).collect();
    let mut best = (0.5f32, 0.0f32);
    for &threshold in &grid {
        let right = index
            .iter()
            .filter(|&&i| (scores[i] >= threshold) == want[i])
            .count();
        let accuracy = right as f32 / index.len() as f32;
        if accuracy > best.1 {
            best = (threshold, accuracy);
        }
    }
    best
}

fn counts(want: &[bool], scores: &[f32], threshold: f32, index: &[usize]) -> (usize, usize) {
    let right = index
        .iter()
        .filter(|&&i| (scores[i] >= threshold) == want[i])
        .count();
    (index.len() - right, right)
}

fn stats(values: &[f64]) -> Value {
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    let min = values.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let sd = (values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n).sqrt();
    json!({"mean": mean, "min": min, "max": max, "sd": sd, "values": values})
}

fn main() {
    let args = parse_args();
    let payload: Value =
        serde_json::from_str(&std::fs::read_to_string(&args.pairs).expect("pairs")).expect("json");
    let items = payload
        .get("pairs")
        .and_then(Value::as_array)
        .expect("pairs 数组");
    let net = base_net(&args);
    let mut pairs = Vec::with_capacity(items.len());
    let mut want = Vec::with_capacity(items.len());
    let mut ids = Vec::with_capacity(items.len());
    for item in items {
        let (a, b) = (floats(item, "a"), floats(item, "b"));
        let teacher = match item.get("label").and_then(Value::as_str) {
            Some("positive") => true,
            Some("negative") => false,
            _ => cos_f32(&a, &b) >= 0.5,
        };
        want.push(teacher);
        ids.push((sentence_id(&a), sentence_id(&b)));
        pairs.push((trit_row(&a), trit_row(&b), teacher));
    }
    let before_scores = scores(&net, &pairs);
    let (edited, changed) = apply_tuned(&args);
    let after_scores = scores(&edited, &pairs);

    let mut rows = Vec::new();
    let mut thresholds = Vec::new();
    let mut edits = Vec::new();
    let mut broken = Vec::new();
    let mut valid_accuracies = Vec::new();
    for repeat in 0..args.repeat {
        let mut groups = Groups::new(&ids);
        let (valid, test) = groups.split(args.valid_ratio, args.split_seed + repeat as u64);
        let (threshold, valid_accuracy) = best_threshold(&want, &before_scores, &valid);
        let (wrong, right) = counts(&want, &before_scores, threshold, &test);
        let (wrong_after, right_after) = counts(&want, &after_scores, threshold, &test);
        let edit_fixed = wrong.saturating_sub(wrong_after);
        let keep_broken = right.saturating_sub(right_after);
        thresholds.push(f64::from(threshold));
        valid_accuracies.push(f64::from(valid_accuracy));
        edits.push(edit_fixed as f64);
        broken.push(keep_broken as f64);
        rows.push(json!({
            "repeat": repeat,
            "split_seed": args.split_seed + repeat as u64,
            "valid_pairs": valid.len(),
            "test_pairs": test.len(),
            "threshold": threshold,
            "valid_accuracy": valid_accuracy,
            "test_baseline": {"wrong": wrong, "right": right},
            "edit_fixed": edit_fixed,
            "keep_broken": keep_broken,
        }));
    }
    let report = json!({
        "arch": args.arch,
        "pairs": pairs.len(),
        "changed": changed,
        "repeat": args.repeat,
        "split_seed": args.split_seed,
        "runs": rows,
        "summary": {
            "threshold": stats(&thresholds),
            "valid_accuracy": stats(&valid_accuracies),
            "edit_fixed": stats(&edits),
            "keep_broken": stats(&broken),
        },
    });
    let text = serde_json::to_string_pretty(&report).expect("json");
    if let Some(path) = &args.out {
        std::fs::write(path, &text).expect("write report");
    }
    println!("{text}");
}
