//! 三值矩阵乘的 GPU 内核（[`matmul.wgsl`]）与同结构 f32 基线同尺对比。
//!
//! 两侧都是 naive「一线程一输出、循环内积维度」——**唯一变量是数据表示**
//! （双导轨 2 bit/trit vs f32 32 bit）。`m×n×k` 的 MAC 数相同，读数按 ns/MAC 报告，
//! `cargo run -p trit-gpu --features gpu` 可复算。没有 adapter 时返回 `None`。
//!
//! 按职责拆分（各自可独立变化）：
//! - [`gpu`]：开设备；
//! - [`inputs`]：确定性矩阵、打包、f32 转置视图；
//! - [`ternary`]：三值内核族（naive / tiled / 2×2 / 4×4 / 3-popc / 2-popc / split-K）；
//! - [`baseline`]：f32 基线族（tiled / 4×4+K32 / 4×4 / 2D / 2×2+K8）。
//!
//! 本文件只保留报告结构与**测量顺序**。顺序刻意集中在这里：同一段内核换个位置
//! 读数能差 25%，历史读数都是按这个顺序取的。

mod baseline;
mod gpu;
mod inputs;
mod ternary;

use gpu::Gpu;
use inputs::Inputs;

pub(crate) use crate::bench::{storage, storage_with, uniform};

/// 一次矩阵乘实验的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct MatmulReport {
    /// 设备名。
    pub adapter: String,
    /// 后端。
    pub backend: String,
    /// m（A 的行数）。
    pub m: usize,
    /// n（B 的列数）。
    pub n: usize,
    /// k（内积维度）。
    pub k: usize,
    /// GPU 结果与 CPU 位并行镜像是否一致。
    pub matches: bool,
    /// 三值内核（双导轨 + 四组 POPC），ns / MAC。
    pub ternary_ns_per_mac: f64,
    /// 三值分块内核（共享内存 16×16 tile / 8-word 段），ns / MAC。
    pub ternary_tiled_ns_per_mac: f64,
    /// f32 分块内核（经典共享内存 tiling），ns / MAC。
    pub f32_tiled_ns_per_mac: f64,
    /// f32 分块内核结果是否与 CPU 镜像一致。
    pub f32_tiled_matches: bool,
    /// 分块三值结果与 CPU 位并行镜像是否一致。
    pub tiled_matches: bool,
    /// 寄存器阻塞三值内核（2×2 微块），ns / MAC。
    pub ternary_reg_ns_per_mac: f64,
    /// 4×4 + K 段 32 的 f32 内核（32×32 tile），ns / MAC。
    pub f32_reg4k32_ns_per_mac: f64,
    /// K 段 16 内核结果是否与 CPU 镜像一致。
    pub f32_reg4k32_matches: bool,
    /// 寄存器阻塞三值结果与 CPU 位并行镜像是否一致。
    pub reg_matches: bool,
    /// 4×4 寄存器阻塞三值内核（32×32 tile），ns / MAC。
    pub ternary_reg4_ns_per_mac: f64,
    /// 4×4 寄存器阻塞 f32 内核（32×32 tile），ns / MAC。
    pub f32_reg4_ns_per_mac: f64,
    /// 4×4 + K 段 8 的 f32 内核结果是否与 CPU 镜像一致。
    pub f32_reg4_matches: bool,
    /// 2D 线程块 naive f32（对齐 cuda-oxide 的映射），ns / MAC。
    pub f32_2d_ns_per_mac: f64,
    /// 2D naive 结果是否与 CPU 镜像一致。
    pub f32_2d_matches: bool,
    /// 2×2 + K 段 8 的 f32 内核（对齐 cuda-oxide reg2），ns / MAC。
    pub f32_reg2k8_ns_per_mac: f64,
    /// K 段 8 内核结果是否与 CPU 镜像一致。
    pub f32_reg2k8_matches: bool,
    /// 4×4 寄存器阻塞三值结果与 CPU 位并行镜像是否一致。
    pub reg4_matches: bool,
    /// 3-popc 内核（2×2 微块，省 1/4 popcount），ns / MAC。
    pub ternary_o3_ns_per_mac: f64,
    /// 3-popc 内核结果与 CPU 位并行镜像是否一致。
    pub o3_matches: bool,

    /// split-K（4 片 + atomicAdd）三值内核，ns / MAC。
    pub ternary_splitk_ns_per_mac: f64,
    /// split-K 结果与 CPU 位并行镜像是否一致。
    pub splitk_matches: bool,
    /// 纯 countOneBits 微基准的吞吐（popc/s）。
    pub popc_ceiling_per_s: f64,
    /// 矩阵乘（三值最优内核）达到该天花板的百分比。
    pub ternary_popc_utilization: f64,
    /// f32 相对误差（|f32−ternary| / max(1,|ternary|) 的最大值）。
    pub f32_relative_error: f64,
    /// fp16 存储变体（vec4<f16> 装载、f32 累加），ns / MAC；adapter 不支持 SHADER_F16 时为 NaN。
    pub f32_f16_ns_per_mac: f64,
    /// fp16 变体相对 f32 CPU 镜像的最大相对误差；不支持时为 NaN。
    pub f32_f16_relative_error: f64,
}

/// 跑一次 `m×k · k×n` 矩阵乘实验；没有可用 adapter 时返回 `None`。
pub fn run(m: usize, k: usize, n: usize) -> Option<MatmulReport> {
    pollster::block_on(run_async(m, k, n))
}

async fn run_async(m: usize, k: usize, n: usize) -> Option<MatmulReport> {
    let gpu = Gpu::open().await?;
    let inp = Inputs::generate(m, k, n);
    let exp = &inp.expected;
    let tern = ternary::Session::new(&gpu, &inp);

    let (ternary_ns_per_mac, matches) = tern.naive(&gpu, exp)?;
    let (ternary_tiled_ns_per_mac, tiled_matches) = tern.tiled(&gpu, exp)?;
    let (ternary_reg_ns_per_mac, reg_matches) = tern.reg2(&gpu, exp)?;
    let (ternary_reg4_ns_per_mac, reg4_matches) = tern.reg4(&gpu, exp)?;
    let (ternary_o3_ns_per_mac, o3_matches) = tern.o3(&gpu, exp)?;
    let (ternary_o2_ns_per_mac, o2_matches) = tern.o2(&gpu, exp)?;
    // 2-popc 必须与 3-popc 给出完全相同的整数结果，否则说明身份变换写错了。
    assert_eq!(
        o2_matches, o3_matches,
        "2-popc 与 3-popc 的正确性必须一致（m={m} n={n} k={k}）"
    );
    eprintln!(
        "2-popc vs 3-popc: {:.4} vs {:.4} ps/MAC ({:+.1}%)",
        ternary_o2_ns_per_mac * 1000.0,
        ternary_o3_ns_per_mac * 1000.0,
        (ternary_o3_ns_per_mac / ternary_o2_ns_per_mac - 1.0) * 100.0
    );

    let base = baseline::Session::new(&gpu, &inp);
    let (f32_tiled_ns_per_mac, f32_tiled_matches) = base.tiled(&gpu, exp)?;
    let (f32_reg4k32_ns_per_mac, f32_reg4k32_matches) = base.reg4k32(&gpu, exp)?;
    let (f32_reg4_ns_per_mac, f32_reg4_matches) = base.reg4(&gpu, exp)?;

    let (ternary_splitk_ns_per_mac, splitk_matches) = tern.splitk(&gpu, exp)?;

    let popc_ceiling_per_s = crate::popc::ceiling(&gpu.device, &gpu.queue, tern.shader())?;

    // 利用率要按各内核自己的 popc 密度算：普通内核每个 word-pair 4 次 popc（32 MAC），
    // 3-popc 内核是 3 次。取所有内核里最高的那个作为「离天花板最近」的读数。
    let utilization = |ns_per_mac: f64, popc_per_word: f64| {
        let macs_per_s = 1.0 / (ns_per_mac * 1e-9);
        (macs_per_s * popc_per_word / 32.0) / popc_ceiling_per_s
    };
    let ternary_popc_utilization = utilization(ternary_o3_ns_per_mac, 3.0)
        .max(utilization(ternary_splitk_ns_per_mac, 4.0))
        .max(utilization(ternary_reg_ns_per_mac, 4.0))
        .max(utilization(ternary_reg4_ns_per_mac, 4.0));

    let (f32_2d_ns_per_mac, f32_2d_matches) = base.two_d(&gpu, exp)?;
    let (f32_reg2k8_ns_per_mac, _) = base.reg2k8(&gpu, exp)?;

    let f32_relative_error = base.relative_error(&gpu, exp)?;
    let f32_reg2k8_matches = f32_relative_error < 1e-3;

    let (f32_f16_ns_per_mac, f32_f16_relative_error) = crate::matmul_f32::run_f16(
        &gpu.device,
        &gpu.queue,
        m,
        n,
        k,
        &inp.fa,
        &inp.fb,
        exp,
        gpu.usage,
    )?;

    Some(MatmulReport {
        adapter: gpu.info.name.clone(),
        backend: format!("{:?}", gpu.info.backend),
        m,
        n,
        k,
        matches,
        ternary_ns_per_mac,
        ternary_tiled_ns_per_mac,
        ternary_reg_ns_per_mac,
        ternary_reg4_ns_per_mac,
        ternary_o3_ns_per_mac,
        f32_tiled_ns_per_mac,
        f32_tiled_matches,
        f32_reg4k32_ns_per_mac,
        f32_reg4k32_matches,
        f32_reg4_ns_per_mac,
        f32_reg4_matches,
        f32_2d_ns_per_mac,
        f32_2d_matches,
        f32_reg2k8_ns_per_mac,
        f32_reg2k8_matches,
        tiled_matches,
        reg_matches,
        reg4_matches,
        o3_matches,
        ternary_splitk_ns_per_mac,
        splitk_matches,
        popc_ceiling_per_s,
        ternary_popc_utilization,
        f32_relative_error,
        f32_f16_ns_per_mac,
        f32_f16_relative_error,
    })
}
