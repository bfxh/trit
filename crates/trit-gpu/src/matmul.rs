//! 三值矩阵乘的 GPU 内核（[`matmul.wgsl`]）与同结构 f32 基线同尺对比。
//!
//! 两侧都是 naive「一线程一输出、循环内积维度」——**唯一变量是数据表示**
//! （双导轨 2 bit/trit vs f32 32 bit）。`m×n×k` 的 MAC 数相同，读数按 ns/MAC 报告，
//! `cargo run -p trit-gpu --features gpu` 可复算。没有 adapter 时返回 `None`。

use std::time::Instant;

use wgpu::util::DeviceExt;

use crate::bench::{median_ns_per, read_u32s};
use crate::{pack_columns, pack_rows};

const WORKGROUP: u32 = 64;

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
    /// 4×4 + K 段 16 的 f32 内核（32×32 tile），ns / MAC。
    pub f32_reg4k16_ns_per_mac: f64,
    /// K 段 16 内核结果是否与 CPU 镜像一致。
    pub f32_reg4k16_matches: bool,
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
}

/// 跑一次 `m×k · k×n` 矩阵乘实验；没有可用 adapter 时返回 `None`。
pub fn run(m: usize, k: usize, n: usize) -> Option<MatmulReport> {
    pollster::block_on(run_async(m, k, n))
}

async fn run_async(m: usize, k: usize, n: usize) -> Option<MatmulReport> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .ok()?;
    let info = adapter.get_info();
    let limits = adapter.limits();
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            required_limits: limits,
            ..Default::default()
        })
        .await
        .ok()?;

    // 确定性随机三值矩阵。
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % 3) as i32 - 1
    };
    let a: Vec<i32> = (0..m * k).map(|_| next()).collect();
    let b: Vec<i32> = (0..k * n).map(|_| next()).collect();
    let (a_pos, a_neg) = pack_rows(&a, k);
    let (b_pos, b_neg) = pack_columns(&b, k, n);
    let expected = crate::matmul_words(m, n, &a_pos, &a_neg, &b_pos, &b_neg);
    let words = k.div_ceil(crate::TRITS_PER_WORD);

    let usage = wgpu::BufferUsages::STORAGE;
    let make = |data: &[u32]| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(data),
            usage: usage | wgpu::BufferUsages::COPY_SRC,
        })
    };
    let a_pos_buf = make(&a_pos);
    let a_neg_buf = make(&a_neg);
    let b_pos_buf = make(&b_pos);
    let b_neg_buf = make(&b_neg);
    let out = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (m * n * 4) as u64,
        usage: usage | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("matmul"),
        source: wgpu::ShaderSource::Wgsl(include_str!("matmul.wgsl").into()),
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[
            storage(0),
            storage(1),
            storage(2),
            storage(3),
            storage_with(4, false),
            uniform(5),
            storage_with(6, false),
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("matmul-ternary"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let pipeline_tiled = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("matmul-ternary-tiled"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("main_tiled"),
        compilation_options: Default::default(),
        cache: None,
    });
    let params: [u32; 3] = [m as u32, n as u32, words as u32];
    let params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&params),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: a_pos_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: a_neg_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: b_pos_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: b_neg_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: out.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: params_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: out.as_entire_binding(),
            },
        ],
    });

    let grid = ((m * n) as u32).div_ceil(WORKGROUP);
    let dispatch = || {
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(grid, 1, 1);
        }
        queue.submit([encoder.finish()]);
    };

    // 预热 + 计时（中位数，与 gpu.rs 同法）。
    for _ in 0..20 {
        dispatch();
    }
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let ternary_ns_per_mac = median_ns_per(&device, m * n * k, dispatch);

    // 读回验证。
    let gpu = read_u32s(&device, &queue, &out, m * n)?;
    // 着色器写 i32、回读按 u32——按补码比较（|k| ≤ 100 远小于 2^31）。
    let matches = gpu
        .iter()
        .zip(expected.iter())
        .all(|(g, e)| i64::from(*g as i32) == *e);

    // 分块内核：同 bind group、2D dispatch（ceil(n/16), ceil(m/16)）。
    let tiled_grid_x = (n as u32).div_ceil(16);
    let tiled_grid_y = (m as u32).div_ceil(16);
    let dispatch_tiled = || {
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&pipeline_tiled);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(tiled_grid_x, tiled_grid_y, 1);
        }
        queue.submit([encoder.finish()]);
    };
    for _ in 0..20 {
        dispatch_tiled();
    }
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let ternary_tiled_ns_per_mac = median_ns_per(&device, m * n * k, dispatch_tiled);
    let tiled_gpu = read_u32s(&device, &queue, &out, m * n)?;
    let tiled_matches = tiled_gpu
        .iter()
        .zip(expected.iter())
        .all(|(g, e)| i64::from(*g as i32) == *e);

    // 寄存器阻塞：workgroup 8×8、每线程 2×2 输出，grid 与 tiled 相同（每组覆盖 16×16）。
    let pipeline_reg = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("matmul-ternary-reg"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("main_tiled2"),
        compilation_options: Default::default(),
        cache: None,
    });
    let dispatch_reg = || {
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&pipeline_reg);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(tiled_grid_x, tiled_grid_y, 1);
        }
        queue.submit([encoder.finish()]);
    };
    for _ in 0..20 {
        dispatch_reg();
    }
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let ternary_reg_ns_per_mac = median_ns_per(&device, m * n * k, dispatch_reg);
    let reg_gpu = read_u32s(&device, &queue, &out, m * n)?;
    let reg_matches = reg_gpu
        .iter()
        .zip(expected.iter())
        .all(|(g, e)| i64::from(*g as i32) == *e);

    // 4×4 寄存器阻塞：32×32 tile、8×8 线程。
    let pipeline_reg4 = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("matmul-ternary-reg4"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("main_tiled4"),
        compilation_options: Default::default(),
        cache: None,
    });
    let reg4_grid_x = (n as u32).div_ceil(32);
    let reg4_grid_y = (m as u32).div_ceil(32);
    let dispatch_reg4 = || {
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&pipeline_reg4);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(reg4_grid_x, reg4_grid_y, 1);
        }
        queue.submit([encoder.finish()]);
    };
    for _ in 0..20 {
        dispatch_reg4();
    }
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let ternary_reg4_ns_per_mac = median_ns_per(&device, m * n * k, dispatch_reg4);
    let reg4_gpu = read_u32s(&device, &queue, &out, m * n)?;
    let reg4_matches = reg4_gpu
        .iter()
        .zip(expected.iter())
        .all(|(g, e)| i64::from(*g as i32) == *e);

    // 3-popc：同 tiled2 结构，把 4 次 popc 压到 3 次（靠双导轨不交叠不变量）。
    let pipeline_o3 = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("matmul-ternary-o3"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("main_tiled2o3"),
        compilation_options: Default::default(),
        cache: None,
    });
    let dispatch_o3 = || {
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&pipeline_o3);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(tiled_grid_x, tiled_grid_y, 1);
        }
        queue.submit([encoder.finish()]);
    };
    for _ in 0..20 {
        dispatch_o3();
    }
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let ternary_o3_ns_per_mac = median_ns_per(&device, m * n * k, dispatch_o3);
    let o3_gpu = read_u32s(&device, &queue, &out, m * n)?;
    let o3_matches = o3_gpu
        .iter()
        .zip(expected.iter())
        .all(|(g, e)| i64::from(*g as i32) == *e);
    // f32 基线（同结构内核，同 m/n/k）。
    let mut fa: Vec<f32> = a.iter().map(|v| *v as f32).collect();
    let mut fb: Vec<f32> = Vec::with_capacity(n * k);
    for col in 0..n {
        for row in 0..k {
            fb.push(b[row * n + col] as f32);
        }
    }
    // vec4 视图要求绑定尺寸是 16 字节的倍数，小矩阵（如 1×1×1）必须补齐。
    fa.resize(fa.len().next_multiple_of(4), 0.0);
    fb.resize(fb.len().next_multiple_of(4), 0.0);
    let fa_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&fa),
        usage,
    });
    let fb_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&fb),
        usage,
    });
    let f32_out = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (m * n * 4) as u64,
        usage: usage | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let f32_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("matmul-f32"),
        source: wgpu::ShaderSource::Wgsl(include_str!("matmul_f32.wgsl").into()),
    });
    let f32_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        // f32 布局：binding 2 是输出，必须 read_write。
        entries: &[
            storage(0),
            storage(1),
            storage_with(2, false),
            uniform(3),
            storage(4),
            storage(5),
        ],
    });
    let f32_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&f32_layout)],
        immediate_size: 0,
    });
    let f32_params: [u32; 3] = [m as u32, n as u32, k as u32];
    let f32_params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&f32_params),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let f32_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &f32_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: fa_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: fb_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: f32_out.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: f32_params_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: fa_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: fb_buf.as_entire_binding(),
            },
        ],
    });
    let f32_pipeline_tiled = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("matmul-f32-tiled"),
        layout: Some(&f32_pipeline_layout),
        module: &f32_shader,
        entry_point: Some("main_tiled"),
        compilation_options: Default::default(),
        cache: None,
    });
    let dispatch_f32_tiled = || {
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&f32_pipeline_tiled);
            pass.set_bind_group(0, &f32_bind, &[]);
            pass.dispatch_workgroups(tiled_grid_x, tiled_grid_y, 1);
        }
        queue.submit([encoder.finish()]);
    };
    for _ in 0..20 {
        dispatch_f32_tiled();
    }
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let f32_tiled_ns_per_mac = median_ns_per(&device, m * n * k, dispatch_f32_tiled);
    let f32_tiled_matches = read_u32s(&device, &queue, &f32_out, m * n)?
        .iter()
        .zip(expected.iter())
        .all(|(bits, want)| {
            (f32::from_bits(*bits) - *want as f32).abs() <= (*want as f32).abs() * 1e-3 + 1e-3
        });

    // 4×4 + K 段 16（屏障减半）。
    let f32_pipeline_reg4k16 = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("matmul-f32-reg4k16"),
        layout: Some(&f32_pipeline_layout),
        module: &f32_shader,
        entry_point: Some("main_tiled4k16"),
        compilation_options: Default::default(),
        cache: None,
    });
    let dispatch_f32_reg4k16 = || {
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&f32_pipeline_reg4k16);
            pass.set_bind_group(0, &f32_bind, &[]);
            pass.dispatch_workgroups(reg4_grid_x, reg4_grid_y, 1);
        }
        queue.submit([encoder.finish()]);
    };
    for _ in 0..20 {
        dispatch_f32_reg4k16();
    }
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let f32_reg4k16_ns_per_mac = median_ns_per(&device, m * n * k, dispatch_f32_reg4k16);
    let f32_reg4k16_out = read_u32s(&device, &queue, &f32_out, m * n)?;
    let f32_reg4k16_matches = f32_reg4k16_out
        .iter()
        .zip(expected.iter())
        .all(|(bits, want)| {
            (f32::from_bits(*bits) - *want as f32).abs() <= (*want as f32).abs() * 1e-3 + 1e-3
        });

    let f32_pipeline_reg4 = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("matmul-f32-reg4"),
        layout: Some(&f32_pipeline_layout),
        module: &f32_shader,
        entry_point: Some("main_tiled4"),
        compilation_options: Default::default(),
        cache: None,
    });
    let dispatch_f32_reg4 = || {
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&f32_pipeline_reg4);
            pass.set_bind_group(0, &f32_bind, &[]);
            pass.dispatch_workgroups(reg4_grid_x, reg4_grid_y, 1);
        }
        queue.submit([encoder.finish()]);
    };
    for _ in 0..20 {
        dispatch_f32_reg4();
    }
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let f32_reg4_ns_per_mac = median_ns_per(&device, m * n * k, dispatch_f32_reg4);
    let f32_reg4_matches = read_u32s(&device, &queue, &f32_out, m * n)?
        .iter()
        .zip(expected.iter())
        .all(|(bits, want)| {
            (f32::from_bits(*bits) - *want as f32).abs() <= (*want as f32).abs() * 1e-3 + 1e-3
        });

    // split-K：沿 K 切 4 片、atomicAdd 汇总；workgroup 数 = tiled × 4。
    let pipeline_splitk = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("matmul-ternary-splitk"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("main_splitk"),
        compilation_options: Default::default(),
        cache: None,
    });
    let dispatch_splitk = || {
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.clear_buffer(&out, 0, None);
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&pipeline_splitk);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(tiled_grid_x, tiled_grid_y, 4);
        }
        queue.submit([encoder.finish()]);
    };
    for _ in 0..20 {
        dispatch_splitk();
    }
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let ternary_splitk_ns_per_mac = median_ns_per(&device, m * n * k, dispatch_splitk);
    let splitk_gpu = read_u32s(&device, &queue, &out, m * n)?;
    let splitk_matches = splitk_gpu
        .iter()
        .zip(expected.iter())
        .all(|(g, e)| i64::from(*g as i32) == *e);

    // ---- popcount 吞吐天花板：只做 countOneBits，判断矩阵乘还剩多少空间 ----
    let popc_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("popc"),
        entries: &[storage_with(0, false), uniform(1)],
    });
    let popc_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&popc_layout)],
        immediate_size: 0,
    });
    let popc_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("popc-ceiling"),
        layout: Some(&popc_pipeline_layout),
        module: &shader,
        entry_point: Some("bench_popc"),
        compilation_options: Default::default(),
        cache: None,
    });
    const POPC_ITERS: u32 = 4096;
    const POPC_GROUPS: u32 = 4096;
    let popc_params: [u32; 2] = [POPC_ITERS, 0x1234_5678];
    let popc_params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&popc_params),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let popc_sink = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 64 * 4,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let popc_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &popc_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: popc_sink.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: popc_params_buf.as_entire_binding(),
            },
        ],
    });
    let dispatch_popc = || {
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&popc_pipeline);
            pass.set_bind_group(0, &popc_bind, &[]);
            pass.dispatch_workgroups(POPC_GROUPS, 1, 1);
        }
        queue.submit([encoder.finish()]);
    };
    for _ in 0..10 {
        dispatch_popc();
    }
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let mut popc_samples: Vec<f64> = Vec::with_capacity(5);
    for _ in 0..5 {
        let start = Instant::now();
        for _ in 0..5 {
            dispatch_popc();
        }
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        popc_samples.push(start.elapsed().as_secs_f64() / 5.0);
    }
    popc_samples.sort_by(|a, b| a.partial_cmp(b).expect("有限值"));
    let popc_seconds = popc_samples[popc_samples.len() / 2].max(1e-12);
    let popc_ops = f64::from(64 * POPC_GROUPS) * f64::from(POPC_ITERS) * 8.0;
    let popc_ceiling_per_s = popc_ops / popc_seconds;

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

    // 2D 线程块 naive（对齐 cuda-oxide 的 16×16 block 映射）。
    let f32_pipeline_2d = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("matmul-f32-2d"),
        layout: Some(&f32_pipeline_layout),
        module: &f32_shader,
        entry_point: Some("main2d"),
        compilation_options: Default::default(),
        cache: None,
    });
    let dispatch_f32_2d = || {
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&f32_pipeline_2d);
            pass.set_bind_group(0, &f32_bind, &[]);
            pass.dispatch_workgroups(tiled_grid_x, tiled_grid_y, 1);
        }
        queue.submit([encoder.finish()]);
    };
    for _ in 0..20 {
        dispatch_f32_2d();
    }
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let f32_2d_ns_per_mac = median_ns_per(&device, m * n * k, dispatch_f32_2d);
    let f32_2d_out = read_u32s(&device, &queue, &f32_out, m * n)?;
    let f32_2d_matches = f32_2d_out.iter().zip(expected.iter()).all(|(bits, want)| {
        (f32::from_bits(*bits) - *want as f32).abs() <= (*want as f32).abs() * 1e-3 + 1e-3
    });

    // 2×2 + K 段 8（对齐 cuda-oxide 的 reg2 结构）。
    let f32_pipeline_k8 = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("matmul-f32-reg2k8"),
        layout: Some(&f32_pipeline_layout),
        module: &f32_shader,
        entry_point: Some("main_tiled2k8"),
        compilation_options: Default::default(),
        cache: None,
    });
    let dispatch_f32_k8 = || {
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&f32_pipeline_k8);
            pass.set_bind_group(0, &f32_bind, &[]);
            pass.dispatch_workgroups(tiled_grid_x, tiled_grid_y, 1);
        }
        queue.submit([encoder.finish()]);
    };
    for _ in 0..20 {
        dispatch_f32_k8();
    }
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let f32_reg2k8_ns_per_mac = median_ns_per(&device, m * n * k, dispatch_f32_k8);

    let f32_result = read_u32s(&device, &queue, &f32_out, m * n)?;
    let f32_bits: Vec<f32> = f32_result
        .iter()
        .map(|word| f32::from_bits(*word))
        .collect();
    let f32_relative_error = f32_bits
        .iter()
        .zip(expected.iter())
        .map(|(f, e)| {
            let scale = e.abs().max(1) as f64;
            f64::from((*f - *e as f32).abs()) / scale
        })
        .fold(0.0f64, f64::max);

    let f32_reg2k8_matches = f32_relative_error < 1e-3;

    Some(MatmulReport {
        adapter: info.name,
        backend: format!("{:?}", info.backend),
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
        f32_reg4k16_ns_per_mac,
        f32_reg4k16_matches,
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
    })
}

pub(crate) use crate::bench::{storage, storage_with, uniform};
