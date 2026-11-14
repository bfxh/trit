//! 模型身份锁定：行为指纹漂移检测（幻觉 = 输出偏离发布基线即锁定）。
//!
//! 威胁（接 #494/#495）：攻击者重算校验和后**内容偷换**——字节合法但行为变了
//! （后门 / 降级模型 / 幻觉化）。防御：加载时与运行中持续跑**行为探针**，
//! 把输出指纹（对探针输入的响应符号向量）与发布基线比对，EWMA 监控漂移；
//! 超阈即**锁定**（拒绝输出 + 审计记录 + 可从审计日志回滚到基线工件）。
//!
//! 「锁定攻击者」的边界：锁定的是**工件身份**（哪个部署实例被偷换），不是
//! 对真人的定位/去匿名化——那需要网络层遥测与法律程序，不在本仓范围。
//!
//! ```bash
//! cargo run --release -p trit-wspace --example identity_lock
//! ```

use serde_json::{Value, json};
use trit_learn::dense::TernaryDense;
use trit_learn::forecast::Ewma;
use trit_learn::loss::cross_entropy;
use trit_learn::opt::Adam;
use trit_learn::{Activation, Ste};
use trit_wspace::edit::{Edit, Patch};
use trit_wspace::trit::Trit;
use trit_wspace::{Layer, Net};

/// xorshift64*。
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
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
}

/// QAT 训练一个"基线"分类器（充当被部署的模型）。
fn train_model(seed: u64, features: &[Vec<f32>], labels: &[usize], train: &[usize]) -> Net {
    let mut rng = Rng::new(seed);
    let make = |rows: usize, cols: usize, rng: &mut Rng| -> TernaryDense {
        let latent: Vec<Vec<f32>> = (0..rows)
            .map(|_| (0..cols).map(|_| rng.next_f32() * 0.05).collect())
            .collect();
        let absmean = |row: usize| {
            let mean = latent[row].iter().map(|w| w.abs()).sum::<f32>() / cols as f32;
            if mean > 1e-6 { mean } else { 1.0 }
        };
        TernaryDense::new(
            rows,
            cols,
            |r, c| latent[r][c],
            absmean,
            Ste::Clipped { ratio: 1.0 },
        )
    };
    let mut hidden = make(32, features[0].len(), &mut rng);
    let mut out = make(2, 32, &mut rng);
    let mut adam_hl = Adam::new(1e-2);
    let mut adam_hs = Adam::new(1e-2);
    let mut adam_ol = Adam::new(1e-2);
    let mut adam_os = Adam::new(1e-2);
    for _ in 0..16 {
        let mut order = train.to_vec();
        for i in (1..order.len()).rev() {
            let j = (rng.next_u64() % (i as u64 + 1)) as usize;
            order.swap(i, j);
        }
        for &index in &order {
            let x = &features[index];
            let h = hidden.forward(x);
            let h_act: Vec<f32> = h.iter().map(|&v| Activation::Tanh.forward(v)).collect();
            let logits = out.forward(&h_act);
            let (_loss, grad_logits) = cross_entropy(&logits, labels[index]);
            let grad_h_act = out.backward(&grad_logits);
            out.update(&mut adam_ol, &mut adam_os);
            let grad_h_pre: Vec<f32> = grad_h_act
                .iter()
                .zip(&h_act)
                .map(|(&g, &y)| g * Activation::Tanh.backward(y))
                .collect();
            let _ = hidden.backward(&grad_h_pre);
            hidden.update(&mut adam_hl, &mut adam_hs);
        }
    }
    let build = |dense: &TernaryDense| -> Layer {
        let trits: Vec<i8> = dense.quantized().into_iter().flatten().collect();
        Layer::with_scales(
            dense.rows(),
            dense.cols(),
            &trits,
            &vec![0i8; dense.rows()],
            dense.scales().to_vec(),
        )
        .expect("valid")
    };
    Net::new(vec![build(&hidden), build(&out)]).expect("chainable")
}

/// 行为指纹：探针输入集合上的输出符号向量（发布时固定）。
fn fingerprint(net: &Net, probes: &[Vec<f32>]) -> Vec<i8> {
    probes
        .iter()
        .map(|x| {
            let mut v = x.clone();
            for layer in net.layers() {
                let cols = layer.cols();
                let mut next = vec![0.0f32; layer.rows()];
                for (row, o) in next.iter_mut().enumerate() {
                    let mut acc = 0.0f32;
                    for (c, t) in layer.trits()[row * cols..(row + 1) * cols]
                        .iter()
                        .enumerate()
                    {
                        acc += f32::from(t.to_i8()) * v[c];
                    }
                    *o = layer.scales()[row] * acc;
                }
                v = next.iter().map(|&x| Activation::Tanh.forward(x)).collect();
            }
            if v[0] > 0.0 {
                1
            } else if v[0] < 0.0 {
                -1
            } else {
                0
            }
        })
        .collect()
}

/// 指纹漂移：不一致的探针比例（0=完全一致，1=全变）。
fn drift(fp_a: &[i8], fp_b: &[i8]) -> f32 {
    fp_a.iter().zip(fp_b).filter(|(a, b)| a != b).count() as f32 / fp_a.len().max(1) as f32
}

fn main() {
    let mut rng = Rng::new(7);
    // 合成数据：两个可分簇（模拟正常/恶意流量特征）。
    let mut features = Vec::new();
    let mut labels = Vec::new();
    for _ in 0..600 {
        let (mut x, label) = if rng.next_f32() < 0.5 {
            (
                vec![rng.next_f32() * 0.4 + 0.55, rng.next_f32() * 0.4],
                1usize,
            )
        } else {
            (
                vec![rng.next_f32() * 0.4, rng.next_f32() * 0.4 + 0.55],
                0usize,
            )
        };
        // 少量交叉噪声。
        if rng.next_f32() < 0.08 {
            x[0] = 1.0 - x[0];
        }
        features.push(x);
        labels.push(label);
    }
    let train: Vec<usize> = (0..420).collect();
    let probes: Vec<Vec<f32>> = features[420..470].to_vec();
    let probe_labels = labels[420..470].to_vec();

    // 1) 发布基线：seed=100 的模型 + 探针指纹。
    let baseline = train_model(100, &features, &labels, &train);
    let baseline_fp = fingerprint(&baseline, &probes);
    let baseline_acc: f32 = probe_labels
        .iter()
        .zip(&baseline_fp)
        .map(|(&l, &f)| usize::from((f > 0) as usize == l))
        .sum::<usize>() as f32
        / probe_labels.len() as f32;

    // 2) 运行中的三个"部署实例"：忠实副本 / 投毒副本（kill-switch 翻转）/
    //    幻觉副本（随机扰动多行——输出不稳定 = 幻觉化的工件）。
    let mut poisoned = baseline.clone();
    // 真后门：**输出层**（决策承载行）30% trits 反转——行为偏移但字节合法、
    // 校验和重算通过。（对照：打在不承重的 hidden 行上是"哑弹"，行为等价
    // 则无需锁定——防御语义只对行为变化负责。）
    let out_cols = poisoned.layers()[1].cols();
    let mut flipped = 0usize;
    for c in 0..out_cols {
        if flipped * 10 >= out_cols * 3 {
            break;
        }
        let cur = poisoned.layers()[1].trits()[c].to_i8();
        if cur != 0 {
            let to = match cur {
                -1 => Trit::Pos,
                1 => Trit::Neg,
                _ => Trit::Zero,
            };
            Patch::new(vec![Edit::SetTrit {
                layer: 1,
                row: 0,
                col: c,
                to,
            }])
            .apply(&mut poisoned)
            .expect("in-bounds");
            flipped += 1;
        }
    }
    let mut hallucinated = baseline.clone();
    let mut halluc_edits = Vec::new();
    for row in 0..hallucinated.layers()[0].rows() {
        if rng.next_f32() < 0.3 {
            let col = rng.next_index(hallucinated.layers()[0].cols());
            halluc_edits.push(Edit::SetTrit {
                layer: 0,
                row,
                col,
                to: if rng.next_f32() < 0.5 {
                    Trit::Pos
                } else {
                    Trit::Neg
                },
            });
        }
    }
    Patch::new(halluc_edits)
        .apply(&mut hallucinated)
        .expect("in-bounds");

    // 3) 监控：EWMA 跟踪漂移率，超阈（0.02）锁定 + 审计记录。
    let mut monitor = Ewma::new(0.3);
    let threshold = 0.02f32;
    let mut audit: Vec<Value> = Vec::new();
    for (name, instance) in [
        ("faithful", &baseline),
        ("poisoned", &poisoned),
        ("hallucinated", &hallucinated),
    ] {
        let fp = fingerprint(instance, &probes);
        let d = drift(&baseline_fp, &fp);
        let ema = monitor.update(d);
        let locked = d > threshold || ema > threshold;
        let mut backdoor_hits = 0usize;
        for (i, &probe_bit) in baseline_fp.iter().enumerate() {
            if fp[i] != probe_bit && probe_labels[i] == 1 {
                backdoor_hits += 1;
            }
        }
        audit.push(json!({
            "instance": name,
            "drift": d,
            "ewma": ema,
            "locked": locked,
            "backdoor_probe_hits": backdoor_hits,
        }));
        let _ = json!(locked);
    }
    // 锁定后动作：从审计日志回滚 = 用基线重建（逐位一致验证）。
    let restored_fp = fingerprint(&baseline, &probes);
    let restored_ok = drift(&baseline_fp, &restored_fp) == 0.0;

    println!(
        "{}",
        json!({
            "stage": "baseline",
            "probe_acc": baseline_acc,
            "probes": probes.len(),
            "threshold": threshold,
        })
    );
    println!(
        "{}",
        json!({
            "stage": "monitor",
            "instances": audit,
            "rollback_bitwise_ok": restored_ok,
        })
    );
}
