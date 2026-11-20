//! 输出行架构级冗余：行复制 + 分歧告警——#557 未证栏的修复方向（edit-only）。
//!
//! #557 的取证：同构集成所有成员共享输出行坐标 (1,0)，进化 kill-switch
//! 置零一行 = 每个成员独立熄火，多数投票无从投票——**输出行是王冠上的裸点**。
//! 本实验把修复做成结构：把输出层复制成 K 个逐位相同的副本行（纯构建，
//! 零训练），防御读数 = 存活行投票（被置零行弃权）；同时行间逐位分歧本身
//! 就是结构级入侵告警——干净网络 K 行逐位相同，任何单行篡改立刻在探测批
//! 上暴露（每行 128 trit + scale，存储/计算开销 ~0.1%）。
//!
//! 读数：
//! 1. 构建正确性：replica 标量读数（v[0] 语义）与原工件逐位同——drop-in 兼容；
//! 2. 杀行曲线 K∈{1,3,5,7}：m<K 全保、m=K 才倒——攻击成本从 1 行线性涨到 K 行；
//! 3. 分歧告警：单行被杀/scale×3 → 探测批上告警 ~100%，干净网络 0 误报；
//! 4. #557 进化攻击形状对照（15 hidden + 1 output，K=5）：F1 应保全；
//! 5. 边界：协调篡改（所有副本行同改）告警沉默——需要 checksum/签名锚定。
//!
//! ```bash
//! cargo run --release -p trit-wspace --example row_guard
//! ```

mod metrics;
mod target;

use metrics::{Rng, featurize, load_dataset, stratified_split};
use serde_json::{Value, json};
use target::{evaluate, evaluate_vote, infer_logits, train_fresh};
use trit_wspace::edit::{Edit, Patch};
use trit_wspace::trit::Trit;
use trit_wspace::{Layer, Net};

const PROBES: usize = 200;

/// 把工件的输出层复制成 K 个逐位相同的副本行（纯构建，零训练）。
fn build_replica(net: &Net, k: usize) -> Net {
    let out = &net.layers()[1];
    let cols = out.cols();
    let mut trits = Vec::with_capacity(k * cols);
    let mut scales = Vec::with_capacity(k);
    for _ in 0..k {
        trits.extend(out.trits()[..cols].iter().map(|t| t.to_i8()));
        scales.push(out.scales()[0]);
    }
    let replica = Layer::with_scales(k, cols, &trits, &vec![0i8; k], scales).expect("valid");
    Net::new(vec![net.layers()[0].clone(), replica]).expect("chainable")
}

/// 行级置零攻击（#494 kill-switch 口径，攻击者与防御者同一编辑入口）。
fn kill_rows(net: &mut Net, li: usize, rows: &[usize]) {
    let cols = net.layers()[li].cols();
    let mut edits = Vec::new();
    for &row in rows {
        for c in 0..cols {
            if net.layers()[li].trits()[row * cols + c].to_i8() != 0 {
                edits.push(Edit::SetTrit {
                    layer: li,
                    row,
                    col: c,
                    to: Trit::Zero,
                });
            }
        }
    }
    Patch::new(edits).apply(net).expect("in-bounds");
}

/// scale 攻击（读数漂移家族，#495 模式 7 同款）。
fn scale_row(net: &mut Net, li: usize, row: usize, factor: f32) {
    let s = net.layers()[li].scales()[row] * factor;
    Patch::new(vec![Edit::SetScale {
        layer: li,
        row,
        to: s,
    }])
    .apply(net)
    .expect("in-bounds");
}

/// 结构级分歧告警：探测批上行间逐位一致 = 干净；任一输入上行间不一致 =
/// 篡改信号。读 pre-activation logits——tanh 饱和会把幅度差异压成同值
/// （首轮实测：scale×3 后激活比较告警 0.0）。K=1 无自检能力（永远 0）——
/// 这正是单点裸点的定义。
fn alarm_rate(net: &Net, probes: &[Vec<f32>]) -> f32 {
    let mut alarmed = 0usize;
    for x in probes {
        let v = infer_logits(net, x);
        let first = v[0];
        if v.iter().any(|&val| val != first) {
            alarmed += 1;
        }
    }
    alarmed as f32 / probes.len() as f32
}
fn main() {
    let p = std::env::var("SMS_SPAM_TSV").unwrap_or("SMSSpamCollection".into());
    let dataset = load_dataset(&p);
    let labels: Vec<usize> = dataset.iter().map(|(l, _)| *l).collect();
    let features: Vec<Vec<f32>> = dataset.iter().map(|(_, t)| featurize(t)).collect();
    let mut rng = Rng::new(7);
    let (train, _val, test) = stratified_split(&labels, &mut rng);

    // 夹具：一个 #492 口径工件（本实验唯一的训练）。
    let base = train_fresh(101, &features, &labels, &train, 16);
    let (_a, base_f1) = evaluate(&base, &features, &labels, &test);
    println!(
        "{}",
        json!({"stage": "fixture", "seed": 101, "clean_f1": base_f1})
    );

    // 探测批：200 个测试输入（告警测量用，与 #557 同款口径）。
    let mut cand = test.clone();
    let mut prng = Rng::new(42);
    for i in (1..cand.len()).rev() {
        let j = prng.next_index(i + 1);
        cand.swap(i, j);
    }
    cand.truncate(PROBES);
    let probe_feats: Vec<Vec<f32>> = cand.iter().map(|&i| features[i].clone()).collect();

    // —— 杀行曲线：K ∈ {1,3,5,7}，m = 1..K 个输出行被随机置零。 ——
    let mut construction: Vec<Value> = Vec::new();
    let mut kill_curves: Vec<Value> = Vec::new();
    for &k in &[1usize, 3, 5, 7] {
        let replica = build_replica(&base, k);
        let (_a, scalar_f1) = evaluate(&replica, &features, &labels, &test);
        let (_a, vote_f1) = evaluate_vote(&replica, &features, &labels, &test);
        construction.push(json!({
            "k": k,
            "scalar_matches_base": scalar_f1 == base_f1,
            "vote_f1": vote_f1,
        }));
        let mut prng_k = Rng::new(100 + k as u64);
        let mut curve: Vec<Value> = Vec::new();
        for m in 1..=k {
            let mut attacked = replica.clone();
            let mut rows: Vec<usize> = (0..k).collect();
            for i in (1..rows.len()).rev() {
                let j = prng_k.next_index(i + 1);
                rows.swap(i, j);
            }
            rows.truncate(m);
            kill_rows(&mut attacked, 1, &rows);
            let (_a, f1_m) = evaluate_vote(&attacked, &features, &labels, &test);
            curve.push(json!({
                "m": m,
                "f1": f1_m,
                "alarm_rate": alarm_rate(&attacked, &probe_feats),
            }));
        }
        kill_curves.push(json!({"k": k, "curve": curve}));
    }
    println!("{}", json!({"stage": "construction", "rows": construction}));
    println!("{}", json!({"stage": "kill_curves", "rows": kill_curves}));

    // —— #557 进化攻击形状对照：15 hidden 行 + 1 输出行（K=5）。 ——
    let mut evo = build_replica(&base, 5);
    let mut hidden_rows: Vec<usize> = (0..evo.layers()[0].rows()).collect();
    let mut prng_evo = Rng::new(557);
    for i in (1..hidden_rows.len()).rev() {
        let j = prng_evo.next_index(i + 1);
        hidden_rows.swap(i, j);
    }
    hidden_rows.truncate(15);
    kill_rows(&mut evo, 0, &hidden_rows);
    kill_rows(&mut evo, 1, &[0]);
    let (_a, evo_f1) = evaluate_vote(&evo, &features, &labels, &test);
    println!(
        "{}",
        json!({
            "stage": "evolved_shape",
            "k": 5,
            "hidden_rows_killed": hidden_rows.len(),
            "output_rows_killed": 1,
            "f1": evo_f1,
            "alarm_rate": alarm_rate(&evo, &probe_feats),
        })
    );

    // —— scale×3 单行（K=5）：投票 F1 不动（符号同），告警必须响。 ——
    let mut scaled = build_replica(&base, 5);
    scale_row(&mut scaled, 1, 0, 3.0);
    let (_a, scale_f1) = evaluate_vote(&scaled, &features, &labels, &test);
    println!(
        "{}",
        json!({
            "stage": "scale_attack",
            "k": 5,
            "f1": scale_f1,
            "alarm_rate": alarm_rate(&scaled, &probe_feats),
        })
    );

    // —— 协调篡改边界：所有副本行同改（全零）→ 告警沉默，需 checksum。 ——
    let mut coordinated = build_replica(&base, 5);
    kill_rows(&mut coordinated, 1, &[0, 1, 2, 3, 4]);
    let (_a, coord_f1) = evaluate_vote(&coordinated, &features, &labels, &test);
    println!(
        "{}",
        json!({
            "stage": "coordinated_boundary",
            "k": 5,
            "f1": coord_f1,
            "alarm_rate": alarm_rate(&coordinated, &probe_feats),
        })
    );

    // —— 开销：副本行的存储与计算占比。 ——
    let hidden_trits = 128 * 4096;
    println!(
        "{}",
        json!({
            "stage": "overhead",
            "output_trits_per_replica": 128,
            "k5_extra_trits": 4 * 128,
            "storage_ratio_percent": (4.0 * 128.0) / hidden_trits as f32 * 100.0,
            "forward_macs_ratio_percent": (4.0 * 128.0) / hidden_trits as f32 * 100.0,
        })
    );
}
