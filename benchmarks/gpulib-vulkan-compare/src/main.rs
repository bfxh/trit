//! gpulib（原生 Vulkan）vs wgpu：同一份三值 matmul WGSL、同一份输入、同一计时口径。
//!
//! 默认形状与 `crates/trit-gpu/README.md` 的读数和 `benchmarks/cuda-oxide-compare`
//! 对齐：`m=512, k=4096, n=512`。

mod runner;
mod spv;

use trit_gpu::matmul::Inputs;

/// 与 `crates/trit-gpu` 的内核逐字相同：直接读那份 WGSL 源码，不复制。
const MATMUL_WGSL: &str = include_str!("../../../crates/trit-gpu/src/matmul.wgsl");

fn main() {
    let (m, k, n, skip_wgpu, wgpu_first) = parse_args();
    println!("形状：m={m} k={k} n={n}｜MAC = {}", m * n * k);

    // 允许把 wgpu 放到前面跑：GPU 时钟/温度会随负载爬升，固定顺序会让后跑的一侧占便宜。
    let mut wgpu_report = None;
    if !skip_wgpu && wgpu_first {
        wgpu_report = run_wgpu(m, k, n);
    }

    let inputs = Inputs::generate(m, k, n);
    let rows = match runner::run(&inputs, MATMUL_WGSL) {
        Ok(rows) => rows,
        Err(err) => {
            eprintln!("gpulib 路径失败：{err}");
            std::process::exit(1);
        }
    };

    println!();
    println!("| 内核 | gpulib ps/MAC | 校验 |");
    println!("|---|---:|---|");
    for row in &rows {
        println!(
            "| {} | {:.3} | {} |",
            row.name,
            row.ps_per_mac,
            if row.matches {
                "逐元素相等"
            } else {
                "**不一致**"
            }
        );
    }

    if skip_wgpu {
        return;
    }

    if wgpu_report.is_none() {
        println!();
        println!("跑 wgpu 对照（trit-gpu::matmul::run，同一形状同一输入）…");
        wgpu_report = run_wgpu(m, k, n);
    }
    let Some(report) = wgpu_report else {
        eprintln!("wgpu 路径没有可用 adapter，跳过对照");
        return;
    };
    println!("wgpu 设备：{}｜后端：{}", report.adapter, report.backend);

    // 只对齐同结构的两组：3-popc（2×2）与 split-K；其余变体在两侧实现细节不同，
    // 不强行配成一行。
    let pairs = [
        (
            "wgsl 3-popc",
            report.ternary_o3_ns_per_mac,
            report.o3_matches,
        ),
        (
            "wgsl split-K x4",
            report.ternary_splitk_ns_per_mac,
            report.splitk_matches,
        ),
    ];
    println!();
    println!("| 内核 | gpulib ps/MAC | wgpu ps/MAC | gpulib/wgpu 时间比 | 两侧校验 |");
    println!("|---|---:|---:|---:|---|");
    for (name, wgpu_ns, wgpu_ok) in pairs {
        let Some(row) = rows.iter().find(|r| r.name == name) else {
            continue;
        };
        let wgpu_ps = wgpu_ns * 1000.0;
        println!(
            "| {} | {:.3} | {:.3} | {:.3}× | {} / {} |",
            name,
            row.ps_per_mac,
            wgpu_ps,
            row.ps_per_mac / wgpu_ps,
            if row.matches { "通过" } else { "失败" },
            if wgpu_ok { "通过" } else { "失败" }
        );
    }
    println!();
    println!("（比值 < 1 表示 gpulib 更快；两侧都是 5 组 × 20 次中位数，warmup 20。）");
}

fn run_wgpu(m: usize, k: usize, n: usize) -> Option<trit_gpu::matmul::MatmulReport> {
    trit_gpu::matmul::run(m, k, n)
}

fn parse_args() -> (usize, usize, usize, bool, bool) {
    let mut m = 512usize;
    let mut k = 4096usize;
    let mut n = 512usize;
    let mut skip_wgpu = false;
    let mut wgpu_first = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--m" => m = parse_usize(args.next(), "--m"),
            "--k" => k = parse_usize(args.next(), "--k"),
            "--n" => n = parse_usize(args.next(), "--n"),
            "--skip-wgpu" => skip_wgpu = true,
            "--wgpu-first" => wgpu_first = true,
            other => {
                eprintln!("未知参数：{other}");
                eprintln!(
                    "用法：cargo run --release -- [--m 512] [--k 4096] [--n 512] [--skip-wgpu] [--wgpu-first]"
                );
                std::process::exit(2);
            }
        }
    }
    (m, k, n, skip_wgpu, wgpu_first)
}

fn parse_usize(value: Option<String>, flag: &str) -> usize {
    value.and_then(|v| v.parse().ok()).unwrap_or_else(|| {
        eprintln!("{flag} 需要一个正整数");
        std::process::exit(2);
    })
}
