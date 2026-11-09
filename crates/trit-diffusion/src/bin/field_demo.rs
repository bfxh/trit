//! 三态场扩散 demo：在 `SignField3D` 上学一组形状，再生成三态场并评估 IoU。
//!
//! 用法：`field_demo [--n 6] [--steps N] [--batch B] [--lr L] [--hidden H]
//!                   [--timesteps T] [--samples S] [--stride K] [--seed S]`

use trit_core::signfield::SignField3D;
use trit_diffusion::data::Xorshift;
use trit_diffusion::field::{
    FieldMlp, FieldTrainOptions, cube, encode, field_dim, inside_iou, ratios, sample_fields,
    sphere, torus, train_field,
};
use trit_diffusion::schedule::{DiffusionSchedule, Schedule};

struct Args {
    n: u32,
    steps: usize,
    batch: usize,
    lr: f32,
    hidden: usize,
    timesteps: usize,
    samples: usize,
    stride: usize,
    seed: u64,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            n: 6,
            steps: 12_000,
            batch: 8,
            lr: 0.01,
            hidden: 64,
            timesteps: 100,
            samples: 5,
            stride: 2,
            seed: 11,
        }
    }
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args::default();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let key = argv[i].as_str();
        let val = argv.get(i + 1).ok_or_else(|| format!("{key} 缺少取值"))?;
        match key {
            "--n" => a.n = val.parse().map_err(|_| "n 不是整数")?,
            "--steps" => a.steps = val.parse().map_err(|_| "steps 不是整数")?,
            "--batch" => a.batch = val.parse().map_err(|_| "batch 不是整数")?,
            "--lr" => a.lr = val.parse().map_err(|_| "lr 不是数值")?,
            "--hidden" => a.hidden = val.parse().map_err(|_| "hidden 不是整数")?,
            "--timesteps" => a.timesteps = val.parse().map_err(|_| "timesteps 不是整数")?,
            "--samples" => a.samples = val.parse().map_err(|_| "samples 不是整数")?,
            "--stride" => a.stride = val.parse().map_err(|_| "stride 不是整数")?,
            "--seed" => a.seed = val.parse().map_err(|_| "seed 不是整数")?,
            other => return Err(format!("未知参数 {other}")),
        }
        i += 2;
    }
    if a.n == 0 || a.steps == 0 || a.batch == 0 || a.hidden == 0 || a.samples == 0 {
        return Err("n/steps/batch/hidden/samples 都必须大于 0".to_string());
    }
    if !a.lr.is_finite() || a.lr <= 0.0 {
        return Err("lr 必须是正的有限值".to_string());
    }
    Ok(a)
}

/// 参考形状集合（名字 + 场）。
fn references(n: u32) -> Vec<(&'static str, SignField3D)> {
    vec![
        ("球 r=1.2", sphere(n, 1.2, 0.6)),
        ("球 r=1.8", sphere(n, 1.8, 0.6)),
        ("球 r=2.4", sphere(n, 2.4, 0.6)),
        ("立方 h=1.4", cube(n, 1.4, 0.6)),
        ("环面 R=1.6 r=0.6", torus(n, 1.6, 0.6, 0.6)),
    ]
}

fn mean_ratios(fields: &[SignField3D]) -> [f32; 3] {
    let mut acc = [0.0f32; 3];
    for f in fields {
        let r = ratios(f);
        for k in 0..3 {
            acc[k] += r[k];
        }
    }
    let n = fields.len().max(1) as f32;
    [acc[0] / n, acc[1] / n, acc[2] / n]
}

fn best_iou(field: &SignField3D, refs: &[SignField3D]) -> f32 {
    refs.iter()
        .map(|r| inside_iou(field, r))
        .fold(0.0f32, f32::max)
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let refs_named = references(args.n);
    let refs: Vec<SignField3D> = refs_named.iter().map(|(_, f)| f.clone()).collect();
    let data: Vec<Vec<f32>> = refs.iter().map(encode).collect();
    let dim = field_dim(args.n);
    let sched = DiffusionSchedule::new(Schedule::Cosine, args.timesteps);
    let opts = FieldTrainOptions {
        steps: args.steps,
        batch: args.batch,
        seed: args.seed,
        log_every: args.steps.div_ceil(5).max(1),
    };

    // 三值模型。
    let mut tern = FieldMlp::new(dim, args.hidden, args.seed, args.lr);
    let t_rep = train_field(&mut tern, &sched, &data, &opts);
    // FP 对照（同初始化、同超参）。
    let mut fp = FieldMlp::fp(dim, args.hidden, args.seed, args.lr);
    let f_rep = train_field(&mut fp, &sched, &data, &opts);

    // 采样并阈值化回三态场。
    let mut rng_t = Xorshift::new(args.seed ^ 0x5151);
    let gen_t: Vec<SignField3D> = sample_fields(&mut tern, &sched, args.samples, args.stride, &mut rng_t)
        .iter()
        .map(|v| trit_diffusion::field::decode(v, args.n, 0.5))
        .collect();
    let mut rng_f = Xorshift::new(args.seed ^ 0x5151);
    let gen_f: Vec<SignField3D> = sample_fields(&mut fp, &sched, args.samples, args.stride, &mut rng_f)
        .iter()
        .map(|v| trit_diffusion::field::decode(v, args.n, 0.5))
        .collect();

    let ious_t: Vec<f32> = gen_t.iter().map(|g| best_iou(g, &refs)).collect();
    let ious_f: Vec<f32> = gen_f.iter().map(|g| best_iou(g, &refs)).collect();
    let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len().max(1) as f32;
    let min = |v: &[f32]| v.iter().fold(f32::INFINITY, |a, &b| a.min(b));
    let max = |v: &[f32]| v.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));

    let rt = mean_ratios(&gen_t);
    let rf = mean_ratios(&gen_f);
    let rr = mean_ratios(&refs);

    println!(
        "## 三态场扩散（n={}，{} 维，{} 个参考形状{}）",
        args.n,
        dim,
        refs.len(),
        if args.stride > 1 {
            format!("，DDIM stride {}", args.stride)
        } else {
            "，DDIM stride 1".to_string()
        }
    );
    println!("| 模型 | 初始 loss | 最终 loss | 权重位宽 | 训练秒 |");
    println!("|---|---:|---:|---:|---:|");
    println!(
        "| 三值（2 bit/权重） | {:.4} | {:.4} | {} | {:.2} |",
        t_rep.initial_loss, t_rep.final_loss, tern.weight_bits(), t_rep.secs
    );
    println!(
        "| FP（32 bit/权重） | {:.4} | {:.4} | {} | {:.2} |",
        f_rep.initial_loss, f_rep.final_loss, fp.weight_bits(), f_rep.secs
    );
    println!();
    println!("## 生成质量（每个样本取与参考形状的最大 IoU）");
    println!("| 模型 | 平均 | 最低 | 最高 |");
    println!("|---|---:|---:|---:|");
    println!(
        "| 三值 | {:.3} | {:.3} | {:.3} |",
        mean(&ious_t),
        min(&ious_t),
        max(&ious_t)
    );
    println!(
        "| FP | {:.3} | {:.3} | {:.3} |",
        mean(&ious_f),
        min(&ious_f),
        max(&ious_f)
    );
    println!();
    println!("## 三态比例（inside / null / outside）");
    println!("| 来源 | inside | null | outside |");
    println!("|---|---:|---:|---:|");
    println!(
        "| 参考平均 | {:.3} | {:.3} | {:.3} |",
        rr[0], rr[1], rr[2]
    );
    println!(
        "| 三值生成 | {:.3} | {:.3} | {:.3} |",
        rt[0], rt[1], rt[2]
    );
    println!(
        "| FP 生成 | {:.3} | {:.3} | {:.3} |",
        rf[0], rf[1], rf[2]
    );

    let json = serde_json::json!({
        "n": args.n,
        "dim": dim,
        "refs": refs.len(),
        "ternary": {
            "initial_loss": t_rep.initial_loss,
            "final_loss": t_rep.final_loss,
            "bits": tern.weight_bits(),
            "secs": t_rep.secs,
            "iou_mean": mean(&ious_t),
            "iou_min": min(&ious_t),
            "iou_max": max(&ious_t),
            "ratios": rt,
        },
        "fp": {
            "initial_loss": f_rep.initial_loss,
            "final_loss": f_rep.final_loss,
            "bits": fp.weight_bits(),
            "secs": f_rep.secs,
            "iou_mean": mean(&ious_f),
            "iou_min": min(&ious_f),
            "iou_max": max(&ious_f),
            "ratios": rf,
        },
    });
    println!("{json}");
}
