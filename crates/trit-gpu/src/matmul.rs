//! 三值矩阵乘的 GPU 内核（[`matmul.wgsl`]）与同结构 f32 基线同尺对比。
//!
//! 两侧都是 naive「一线程一输出、循环内积维度」——**唯一变量是数据表示**
//! （双导轨 2 bit/trit vs f32 32 bit）。`m×n×k` 的 MAC 数相同，读数按 ns/MAC 报告，
//! `cargo run -p trit-gpu --features gpu` 可复算。没有 adapter 时返回 `None`。

use std::time::Instant;

use wgpu::util::DeviceExt;

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
    /// 同结构 f32 内核，ns / MAC。
    pub f32_ns_per_mac: f64,
    /// f32 分块内核（经典共享内存 tiling），ns / MAC。
    pub f32_tiled_ns_per_mac: f64,
    /// 分块三值结果与 CPU 位并行镜像是否一致。
    pub tiled_matches: bool,
    /// 寄存器阻塞三值内核（2×2 微块），ns / MAC。
    pub ternary_reg_ns_per_mac: f64,
    /// 寄存器阻塞 f32 内核，ns / MAC。
    pub f32_reg_ns_per_mac: f64,
    /// 寄存器阻塞三值结果与 CPU 位并行镜像是否一致。
    pub reg_matches: bool,
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
        usage: usage | wgpu::BufferUsages::COPY_SRC,
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

    // f32 基线（同结构内核，同 m/n/k）。
    let fa: Vec<f32> = a.iter().map(|v| *v as f32).collect();
    let mut fb: Vec<f32> = Vec::with_capacity(n * k);
    for col in 0..n {
        for row in 0..k {
            fb.push(b[row * n + col] as f32);
        }
    }
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
        entries: &[storage(0), storage(1), storage_with(2, false), uniform(3)],
    });
    let f32_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&f32_layout)],
        immediate_size: 0,
    });
    let f32_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("matmul-f32"),
        layout: Some(&f32_pipeline_layout),
        module: &f32_shader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
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
        ],
    });
    let dispatch_f32 = || {
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&f32_pipeline);
            pass.set_bind_group(0, &f32_bind, &[]);
            pass.dispatch_workgroups(grid, 1, 1);
        }
        queue.submit([encoder.finish()]);
    };
    for _ in 0..20 {
        dispatch_f32();
    }
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let f32_ns_per_mac = median_ns_per(&device, m * n * k, dispatch_f32);

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

    let f32_pipeline_reg = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("matmul-f32-reg"),
        layout: Some(&f32_pipeline_layout),
        module: &f32_shader,
        entry_point: Some("main_tiled2"),
        compilation_options: Default::default(),
        cache: None,
    });
    let dispatch_f32_reg = || {
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&f32_pipeline_reg);
            pass.set_bind_group(0, &f32_bind, &[]);
            pass.dispatch_workgroups(tiled_grid_x, tiled_grid_y, 1);
        }
        queue.submit([encoder.finish()]);
    };
    for _ in 0..20 {
        dispatch_f32_reg();
    }
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let f32_reg_ns_per_mac = median_ns_per(&device, m * n * k, dispatch_f32_reg);

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
        f32_ns_per_mac,
        f32_tiled_ns_per_mac,
        f32_reg_ns_per_mac,
        tiled_matches,
        reg_matches,
        f32_relative_error,
    })
}

pub(crate) fn storage(binding: u32) -> wgpu::BindGroupLayoutEntry {
    storage_with(binding, true)
}

pub(crate) fn storage_with(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

pub(crate) fn uniform(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn median_ns_per(device: &wgpu::Device, macs: usize, mut submit: impl FnMut()) -> f64 {
    let iters = 20;
    let mut samples = Vec::with_capacity(5);
    for _ in 0..5 {
        let start = Instant::now();
        for _ in 0..iters {
            submit();
        }
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        samples.push(start.elapsed().as_nanos() as f64 / (iters as f64 * macs as f64));
    }
    samples.sort_by(|a, b| a.partial_cmp(b).expect("有限值"));
    samples[samples.len() / 2]
}

fn read_u32s(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buffer: &wgpu::Buffer,
    count: usize,
) -> Option<Vec<u32>> {
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (count * 4) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, (count * 4) as u64);
    queue.submit([encoder.finish()]);
    let slice = staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let mapped = slice.get_mapped_range().expect("map");
    let (chunks, _) = mapped.as_chunks::<4>();
    Some(
        chunks
            .iter()
            .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            .collect(),
    )
}
