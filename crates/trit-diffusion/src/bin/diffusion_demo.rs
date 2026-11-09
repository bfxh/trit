//! 三值 vs FP 扩散模型对比 demo。
//!
//! 用法：`diffusion_demo [--steps N] [--batch B] [--lr L] [--seed S] [--hidden H]
//!                        [--timesteps T] [--samples N] [--ddim STRIDE]`
//!
//! 同初始化、同超参训练两个去噪器（三值权重 / FP 权重），比较：
//! 训练损失、权重位宽、耗时，以及采样点的两模态占比。

use trit_diffusion::data::{Xorshift, sample_ring};
use trit_diffusion::denoiser::TernaryMlp;
use trit_diffusion::sample::{ddim_sample, ring_stats};
use trit_diffusion::schedule::{DiffusionSchedule, Schedule};
use trit_diffusion::train::{TrainOptions, train};

struct Args {
    steps: usize,
    batch: usize,
    lr: f32,
    seed: u64,
    hidden: usize,
    timesteps: usize,
    samples: usize,
    ddim: usize,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            steps: 1500,
            batch: 64,
            lr: 0.02,
            seed: 42,
            hidden: 32,
            timesteps: 200,
            samples: 1000,
            // 默认 DDIM：DDPM 在短步数（200）cosine 调度下末端 β→0.999、`1/√α≈31.6`，
            // 每步放大预测误差，实测采样半径会漂到 1e3 量级（三值与 FP 都如此）。
            ddim: 1,
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
            "--steps" => a.steps = val.parse().map_err(|_| "steps 不是整数")?,
            "--batch" => a.batch = val.parse().map_err(|_| "batch 不是整数")?,
            "--lr" => a.lr = val.parse().map_err(|_| "lr 不是数值")?,
            "--seed" => a.seed = val.parse().map_err(|_| "seed 不是整数")?,
            "--hidden" => a.hidden = val.parse().map_err(|_| "hidden 不是整数")?,
            "--timesteps" => a.timesteps = val.parse().map_err(|_| "timesteps 不是整数")?,
            "--samples" => a.samples = val.parse().map_err(|_| "samples 不是整数")?,
            "--ddim" => a.ddim = val.parse().map_err(|_| "ddim 不是整数")?,
            other => return Err(format!("未知参数 {other}")),
        }
        i += 2;
    }
    if a.steps == 0 || a.batch == 0 || a.hidden == 0 || a.samples == 0 || a.timesteps == 0 {
        return Err("steps/batch/hidden/samples/timesteps 都必须大于 0".to_string());
    }
    if !a.lr.is_finite() || a.lr <= 0.0 {
        return Err("lr 必须是正的有限值".to_string());
    }
    Ok(a)
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let sched = DiffusionSchedule::new(Schedule::Cosine, args.timesteps);
    let mut data_rng = Xorshift::new(args.seed);
    let data = sample_ring(4096, &mut data_rng);
    let opts = TrainOptions {
        steps: args.steps,
        batch: args.batch,
        seed: args.seed ^ 0x9e37_79b9,
        log_every: args.steps.div_ceil(6).max(1),
    };

    // 三值模型。
    let mut tern = TernaryMlp::new(args.hidden, args.seed, args.lr);
    let t0 = std::time::Instant::now();
    let t_report = train(&mut tern, &sched, &data, &opts);
    let t_secs = t0.elapsed().as_secs_f32();

    // FP 对照（同初始化、同超参）。
    let mut fp = TernaryMlp::fp(args.hidden, args.seed, args.lr);
    let f0 = std::time::Instant::now();
    let f_report = train(&mut fp, &sched, &data, &opts);
    let f_secs = f0.elapsed().as_secs_f32();

    // 采样。
    let mut rng_t = Xorshift::new(args.seed.wrapping_add(1));
    let pts_t = if args.ddim > 0 {
        ddim_sample(&mut tern, &sched, args.samples, args.ddim, 0.0, &mut rng_t)
    } else {
        trit_diffusion::ddpm_sample(&mut tern, &sched, args.samples, &mut rng_t)
    };
    let st = ring_stats(&pts_t);
    let mut rng_f = Xorshift::new(args.seed.wrapping_add(1));
    let pts_f = if args.ddim > 0 {
        ddim_sample(&mut fp, &sched, args.samples, args.ddim, 0.0, &mut rng_f)
    } else {
        trit_diffusion::ddpm_sample(&mut fp, &sched, args.samples, &mut rng_f)
    };
    let sf = ring_stats(&pts_f);

    // 数据分布基线，供对照。
    let sd = ring_stats(&data);

    println!("## 训练（步 {} / 批 {} / lr {} / 隐层 {} / 扩散步 {}）", args.steps, args.batch, args.lr, args.hidden, args.timesteps);
    println!("| 模型 | 初始 loss | 最终 loss | 权重位宽 | 训练秒 |");
    println!("|---|---:|---:|---:|---:|");
    println!(
        "| 三值（2 bit/权重） | {:.4} | {:.4} | {} | {:.2} |",
        t_report.initial_loss, t_report.final_loss, t_report.weight_bits, t_secs
    );
    println!(
        "| FP（32 bit/权重） | {:.4} | {:.4} | {} | {:.2} |",
        f_report.initial_loss, f_report.final_loss, f_report.weight_bits, f_secs
    );
    println!();
    println!("## 损失曲线（三值 / FP）");
    let tt: Vec<String> = t_report.loss_curve.iter().map(|v| format!("{v:.3}")).collect();
    let ff: Vec<String> = f_report.loss_curve.iter().map(|v| format!("{v:.3}")).collect();
    println!("- 三值：{}", tt.join(" → "));
    println!("- FP：  {}", ff.join(" → "));
    println!();
    println!("## 采样（{} 点{}）", args.samples, if args.ddim > 0 { format!("，DDIM stride {}", args.ddim) } else { "，DDPM".to_string() });
    println!("| 来源 | 环上占比 | 盘内占比 | 平均半径 | r p10/p90 |");
    println!("|---|---:|---:|---:|---|");
    println!(
        "| 真实数据 | {:.2} | {:.2} | {:.3} | {:.2}/{:.2} |",
        sd.ring_frac, sd.disk_frac, sd.mean_norm, sd.p10_r, sd.p90_r
    );
    println!(
        "| 三值模型 | {:.2} | {:.2} | {:.3} | {:.2}/{:.2} |",
        st.ring_frac, st.disk_frac, st.mean_norm, st.p10_r, st.p90_r
    );
    println!(
        "| FP 模型 | {:.2} | {:.2} | {:.3} | {:.2}/{:.2} |",
        sf.ring_frac, sf.disk_frac, sf.mean_norm, sf.p10_r, sf.p90_r
    );
    println!();
    println!(
        "{}",
        if st.both_modes_present() && sf.both_modes_present() {
            "✓ 三值与 FP 都保住了两个模态（无模态塌缩）"
        } else if st.both_modes_present() {
            "△ 只有三值保住了两个模态"
        } else {
            "✗ 出现模态塌缩或分布失衡"
        }
    );

    let json = serde_json::json!({
        "ternary": {
            "initial_loss": t_report.initial_loss,
            "final_loss": t_report.final_loss,
            "bits": t_report.weight_bits,
            "secs": t_secs,
            "ring_frac": st.ring_frac,
            "disk_frac": st.disk_frac,
        },
        "fp": {
            "initial_loss": f_report.initial_loss,
            "final_loss": f_report.final_loss,
            "bits": f_report.weight_bits,
            "secs": f_secs,
            "ring_frac": sf.ring_frac,
            "disk_frac": sf.disk_frac,
        },
    });
    println!("{json}");
}
