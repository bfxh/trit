//! 用 gpulib 的 Vulkan 后端跑 trit-gpu 的三值 matmul 内核族。
//!
//! 与 trit-gpu 的 wgpu 路径共用同一份 WGSL、同一份输入、同一套 CPU 期望值；
//! 唯一变量是后端（wgpu → Vulkan 抽象层 vs ash 直接构造命令缓冲）。

use std::time::Instant;

use gpulib_core::{BindingKind, Buffer, Device, Kernel, KernelSource};
use gpulib_vulkan::VulkanDevice;
use trit_gpu::matmul::Inputs;

use crate::spv::{CLEAR_WGSL, to_spirv};

/// 一次内核测量的结果。
pub struct Row {
    pub name: &'static str,
    /// 皮秒 / MAC（与 trit-gpu README 同口径）。
    pub ps_per_mac: f64,
    /// 读回结果与 CPU 位并行镜像是否逐元素相等。
    pub matches: bool,
}

struct Variant {
    name: &'static str,
    entry: &'static str,
    workgroup: [u32; 3],
    grid: [u32; 3],
    /// split-K 的 atomicAdd 需要每轮先清输出。
    clear: bool,
}

/// 跑全部变体；设备不可用时返回 Err（调用方打印并退出）。
pub fn run(inputs: &Inputs, wgsl: &str) -> Result<Vec<Row>, String> {
    let dev = VulkanDevice::new().map_err(|e| format!("打开 Vulkan 设备失败：{e}"))?;
    println!("gpulib 设备：{}", dev.name());

    let spv = to_spirv(wgsl)?;
    let clear_spv = to_spirv(CLEAR_WGSL)?;

    let (m, n, k) = (inputs.m, inputs.n, inputs.k);
    let macs = m * n * k;
    let elems = m * n;

    let upload = |data: &[u8]| -> Result<Buffer, String> {
        let buffer = dev
            .create_buffer(data.len())
            .map_err(|e| format!("分配 buffer 失败：{e}"))?;
        dev.upload(buffer, data)
            .map_err(|e| format!("上传 buffer 失败：{e}"))?;
        Ok(buffer)
    };
    let bytes_u32 = |v: &[u32]| -> Vec<u8> { v.iter().flat_map(|x| x.to_le_bytes()).collect() };

    let a_pos = upload(&bytes_u32(&inputs.a_pos))?;
    let a_neg = upload(&bytes_u32(&inputs.a_neg))?;
    let b_pos = upload(&bytes_u32(&inputs.b_pos))?;
    let b_neg = upload(&bytes_u32(&inputs.b_neg))?;
    let out = dev
        .create_buffer(elems * 4)
        .map_err(|e| format!("分配 out 失败：{e}"))?;

    let mut params = Vec::with_capacity(12);
    params.extend((m as u32).to_le_bytes());
    params.extend((n as u32).to_le_bytes());
    params.extend((inputs.words as u32).to_le_bytes());
    let params_buf = upload(&params)?;

    let bindings = vec![
        BindingKind::Storage,
        BindingKind::Storage,
        BindingKind::Storage,
        BindingKind::Storage,
        BindingKind::Storage,
        BindingKind::Uniform,
        BindingKind::Storage,
    ];
    let compile = |words: &[u32], entry: &str, workgroup: [u32; 3], kinds: &[BindingKind]| {
        dev.compile(&Kernel {
            source: KernelSource::SpirV(words.to_vec()),
            entry: entry.to_string(),
            workgroup,
            bindings: kinds.to_vec(),
        })
        .map_err(|e| format!("编译 {entry} 失败：{e}"))
    };

    let clear_pipe = compile(&clear_spv, "main", [64, 1, 1], &[BindingKind::Storage])?;
    let clear_grid = [(elems as u32).div_ceil(64), 1, 1];

    let tiled = [(n as u32).div_ceil(16), (m as u32).div_ceil(16), 1];
    let reg4 = [(n as u32).div_ceil(32), (m as u32).div_ceil(32), 1];
    let variants = [
        Variant {
            name: "wgsl naive",
            entry: "main",
            workgroup: [64, 1, 1],
            grid: [((m * n) as u32).div_ceil(64), 1, 1],
            clear: false,
        },
        Variant {
            name: "wgsl tiled 16x16",
            entry: "main_tiled",
            workgroup: [16, 16, 1],
            grid: tiled,
            clear: false,
        },
        Variant {
            name: "wgsl reg 2x2",
            entry: "main_tiled2",
            workgroup: [8, 8, 1],
            grid: tiled,
            clear: false,
        },
        Variant {
            name: "wgsl reg 4x4",
            entry: "main_tiled4",
            workgroup: [8, 8, 1],
            grid: reg4,
            clear: false,
        },
        Variant {
            name: "wgsl 3-popc",
            entry: "main_tiled2o3",
            workgroup: [8, 8, 1],
            grid: tiled,
            clear: false,
        },
        Variant {
            name: "wgsl split-K x4",
            entry: "main_splitk",
            workgroup: [8, 8, 1],
            grid: [tiled[0], tiled[1], 4],
            clear: true,
        },
    ];

    let mut rows = Vec::with_capacity(variants.len());
    for v in &variants {
        let pipe = compile(&spv, v.entry, v.workgroup, &bindings)?;
        let buffers = [a_pos, a_neg, b_pos, b_neg, out, params_buf, out];
        rows.push(measure(
            &dev,
            v,
            pipe,
            clear_pipe,
            clear_grid,
            &buffers,
            out,
            &inputs.expected,
            macs,
        )?);
    }

    Ok(rows)
}

#[allow(clippy::too_many_arguments)]
fn measure(
    dev: &VulkanDevice,
    v: &Variant,
    pipe: gpulib_core::PipelineId,
    clear_pipe: gpulib_core::PipelineId,
    clear_grid: [u32; 3],
    buffers: &[Buffer; 7],
    out: Buffer,
    expected: &[i64],
    macs: usize,
) -> Result<Row, String> {
    let submit = || -> Result<(), String> {
        if v.clear {
            dev.dispatch(clear_pipe, &[out], clear_grid)
                .map_err(|e| format!("派发 {} 失败：{e}", v.entry))?;
        }
        dev.dispatch(pipe, buffers, v.grid)
            .map_err(|e| format!("派发 {} 失败：{e}", v.entry))
    };

    // 预热 20 次；命令累积到 sync 一次提交。
    for _ in 0..20 {
        submit()?;
    }
    dev.sync().map_err(|e| format!("同步失败：{e}"))?;

    // 5 组 × 20 次，取中位；每组的 20 次 dispatch 累积后一次 sync。
    let mut samples = Vec::with_capacity(5);
    for _ in 0..5 {
        let start = Instant::now();
        for _ in 0..20 {
            submit()?;
        }
        dev.sync().map_err(|e| format!("同步失败：{e}"))?;
        samples.push(start.elapsed().as_nanos() as f64 / (20.0 * macs as f64));
    }
    samples.sort_by(|a, b| a.partial_cmp(b).expect("有限值"));
    let ps_per_mac = samples[samples.len() / 2] * 1000.0;

    // 校验：清输出 → 跑一次 → 读回逐元素比较。
    dev.dispatch(clear_pipe, &[out], clear_grid)
        .map_err(|e| format!("清输出失败：{e}"))?;
    submit()?;
    dev.sync().map_err(|e| format!("同步失败：{e}"))?;
    let raw = dev.download(out).map_err(|e| format!("回读失败：{e}"))?;
    let matches = raw
        .as_chunks::<4>()
        .0
        .iter()
        .zip(expected.iter())
        .all(|(c, e)| i32::from_le_bytes(*c) as i64 == *e)
        && raw.len() == expected.len() * 4;

    Ok(Row {
        name: v.name,
        ps_per_mac,
        matches,
    })
}
