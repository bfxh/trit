//! `trit-gpu bench [--trits N]`：有 GPU 就跑，没有就提示。

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("bench") {
        eprintln!("用法：trit-gpu bench [--trits N]（需要 --features gpu 构建）");
        return ExitCode::FAILURE;
    }
    let trits = args
        .iter()
        .position(|arg| arg == "--trits")
        .and_then(|index| args.get(index + 1))
        .and_then(|raw| raw.parse::<usize>().ok())
        .unwrap_or(1 << 20);
    match bench(trits) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(feature = "gpu")]
fn bench(trits: usize) -> Result<(), String> {
    let report = trit_gpu::gpu::run(trits).ok_or("没有可用的 GPU adapter")?;
    let gpu_speedup = report.gpu_f32_ns_per_element / report.gpu_ns_per_trit;
    let cpu_speedup = report.cpu_f32_ns_per_element / report.cpu_ns_per_trit;
    println!(
        "三值 vs 二进制（1 号 = 三值，2 号 = 二进制）  设备：{} / {}  规模：{} trit",
        report.adapter, report.backend, report.trits
    );
    println!(
        "  GPU 1 号（三值）      {:>10.5} ps/trit",
        report.gpu_ns_per_trit * 1000.0
    );
    println!(
        "    · B 只合并 POPC（2 组 + 每字 atomic）{:>10.5} ps/trit（{:+.1}%）",
        report.gpu_popc2_ns_per_trit * 1000.0,
        (report.gpu_ns_per_trit / report.gpu_popc2_ns_per_trit - 1.0) * 100.0
    );
    println!(
        "    · C 合并 POPC + workgroup 归约      {:>10.5} ps/trit（{:+.1}%）",
        report.gpu_reduce_ns_per_trit * 1000.0,
        (report.gpu_ns_per_trit / report.gpu_reduce_ns_per_trit - 1.0) * 100.0
    );
    println!(
        "  CPU 1 号（三值）      {:>10.5} ps/trit",
        report.cpu_ns_per_trit * 1000.0
    );
    println!(
        "  GPU 2 号（二值 f32）  {:>10.5} ps/元素",
        report.gpu_f32_ns_per_element * 1000.0
    );
    println!(
        "  CPU 2 号（二值 f32）  {:>10.5} ps/元素",
        report.cpu_f32_ns_per_element * 1000.0
    );
    println!("  同设备加速（1 号 / 2 号）：GPU {gpu_speedup:.1}×，CPU {cpu_speedup:.1}×");
    println!(
        "  校验：GPU {} == CPU {}：{}",
        report.gpu, report.expected, report.matches
    );
    let obj = serde_json::json!({
        "adapter": report.adapter,
        "backend": report.backend,
        "trits": report.trits,
        "numbering": {"1": "ternary", "2": "binary"},
        "rows": [
            {"label": "GPU 1", "device": "gpu", "base": "ternary", "ns_per_element": report.gpu_ns_per_trit},
            {"label": "CPU 1", "device": "cpu", "base": "ternary", "ns_per_element": report.cpu_ns_per_trit},
            {"label": "GPU 2", "device": "gpu", "base": "f32", "ns_per_element": report.gpu_f32_ns_per_element},
            {"label": "CPU 2", "device": "cpu", "base": "f32", "ns_per_element": report.cpu_f32_ns_per_element},
        ],
        "speedup_1_over_2": {"gpu": gpu_speedup, "cpu": cpu_speedup},
        "expected": report.expected,
        "gpu_value": report.gpu,
        "matches": report.matches,
        "float_relative_error": report.float_relative_error,
    });
    println!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());

    // 矩阵乘内核：同结构 naive，唯一变量是数据表示（2 bit/trit vs f32）。
    let Some(matmul) = trit_gpu::matmul::run(512, 4096, 512) else {
        return Ok(());
    };
    let matmul_speedup = matmul.f32_ns_per_mac / matmul.ternary_ns_per_mac;
    println!(
        "矩阵乘（{}，{}×{}×{}；naive 与 tiled 同尺）",
        matmul.adapter, matmul.m, matmul.k, matmul.n
    );
    println!(
        "  GPU 1 号（三值双导轨）naive {:>10.5}  tiled {:>10.5}  reg2×2 {:>10.5}  reg4×4 {:>10.5}  vec2 {:>10.5}  split-K {:>10.5}  w16 {:>10.5} ps/MAC",
        matmul.ternary_ns_per_mac * 1000.0,
        matmul.ternary_tiled_ns_per_mac * 1000.0,
        matmul.ternary_reg_ns_per_mac * 1000.0,
        matmul.ternary_reg4_ns_per_mac * 1000.0,
        matmul.ternary_vec_ns_per_mac * 1000.0,
        matmul.ternary_splitk_ns_per_mac * 1000.0,
        matmul.ternary_w16_ns_per_mac * 1000.0
    );
    println!(
        "  GPU 2 号（二值 f32）  naive {:>10.5}  tiled {:>10.5}  reg2×2 {:>10.5}  reg4×4 {:>10.5} ps/MAC",
        matmul.f32_ns_per_mac * 1000.0,
        matmul.f32_tiled_ns_per_mac * 1000.0,
        matmul.f32_reg_ns_per_mac * 1000.0,
        matmul.f32_reg4_ns_per_mac * 1000.0
    );
    let tiled_speedup = matmul.f32_tiled_ns_per_mac / matmul.ternary_tiled_ns_per_mac;
    let reg_speedup = matmul.f32_reg_ns_per_mac / matmul.ternary_reg_ns_per_mac;
    let reg4_speedup = matmul.f32_reg4_ns_per_mac / matmul.ternary_reg4_ns_per_mac;
    let best_ternary = matmul
        .ternary_reg_ns_per_mac
        .min(matmul.ternary_reg4_ns_per_mac)
        .min(matmul.ternary_vec_ns_per_mac)
        .min(matmul.ternary_splitk_ns_per_mac)
        .min(matmul.ternary_w16_ns_per_mac);
    let best_f32 = matmul.f32_reg_ns_per_mac.min(matmul.f32_reg4_ns_per_mac);
    let best_speedup = best_f32 / best_ternary;
    println!(
        "  最优对最优：reg4×4 {reg4_speedup:.1}× / reg2×2 {reg_speedup:.1}× / tiled {tiled_speedup:.1}× / naive {matmul_speedup:.1}×；当前最优 {best_speedup:.1}×"
    );
    println!(
        "  POPC 天花板 {:.2e}/s；三值最优内核达到 {:.0}%（还剩 {:.1}× 理论空间）",
        matmul.popc_ceiling_per_s,
        matmul.ternary_popc_utilization * 100.0,
        1.0 / matmul.ternary_popc_utilization.max(1e-9)
    );
    println!(
        "  校验：naive {} / tiled {} / reg2×2 {} / reg4×4 {} / vec2 {} / split-K {} / w16 {}（f32 相对误差 {:.2e}）",
        matmul.matches,
        matmul.tiled_matches,
        matmul.reg_matches,
        matmul.reg4_matches,
        matmul.vec_matches,
        matmul.splitk_matches,
        matmul.w16_matches,
        matmul.f32_relative_error
    );
    Ok(())
}

#[cfg(not(feature = "gpu"))]
fn bench(_trits: usize) -> Result<(), String> {
    Err(
        "这个二进制没有 GPU 后端；用 `cargo run -p trit-gpu --features gpu -- bench` 构建"
            .to_string(),
    )
}
