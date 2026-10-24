//! MOD 模型前向的 GPU 端到端入口：吃**预打包**的双轨缓冲。
//!
//! 与 [`crate::matmul`] 的差别：`matmul::run` 内部生成数据并打包；本入口把打包
//! 留给调用方——trit-texture 的 `TritModelLayer::to_dual_rail()` 输出即可直接
//! 作为 A 侧缓冲上传（「GPU 兼容」的端到端证明路径，真机测试
//! `tests/model_forward.rs` 钉住）。没有 adapter 时返回 `None`。

use wgpu::util::DeviceExt;

use crate::matmul::{storage, storage_with, uniform};

/// naive 内核的工作组大小（与 `matmul.wgsl` 的 `@workgroup_size(64)` 一致）。
const WORKGROUP: u32 = 64;

/// 用 naive matmul 内核（[`crate::matmul.wgsl`] 的 `main` 入口）跑一次预打包
/// 三值矩阵乘。
///
/// - `a_pos/a_neg`：`m` 行 × `words` 字，行主序（`pack_rows` / `to_dual_rail` 布局）；
/// - `b_pos/b_neg`：`n` 列 × `words` 字，转置行主序（`pack_columns` 布局）；
/// - 返回 `m × n` 行主序的 i32 结果。
pub fn matmul_packed(
    m: usize,
    n: usize,
    words: usize,
    a_pos: &[u32],
    a_neg: &[u32],
    b_pos: &[u32],
    b_neg: &[u32],
) -> Option<Vec<i32>> {
    pollster::block_on(matmul_packed_async(m, n, words, a_pos, a_neg, b_pos, b_neg))
}

async fn matmul_packed_async(
    m: usize,
    n: usize,
    words: usize,
    a_pos: &[u32],
    a_neg: &[u32],
    b_pos: &[u32],
    b_neg: &[u32],
) -> Option<Vec<i32>> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .ok()?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await
        .ok()?;

    let usage = wgpu::BufferUsages::STORAGE;
    let make = |data: &[u32]| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(data),
            usage: usage | wgpu::BufferUsages::COPY_SRC,
        })
    };
    let a_pos_buf = make(a_pos);
    let a_neg_buf = make(a_neg);
    let b_pos_buf = make(b_pos);
    let b_neg_buf = make(b_neg);
    let out = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (m * n * 4) as u64,
        usage: usage | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("matmul-forward"),
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
        label: Some("matmul-forward"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("main"),
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
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(grid, 1, 1);
    }
    queue.submit([encoder.finish()]);

    // 读回（i32 结果按小端还原）。
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (m * n * 4) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    encoder.copy_buffer_to_buffer(&out, 0, &staging, 0, (m * n * 4) as u64);
    queue.submit([encoder.finish()]);
    let slice = staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
    let data = slice.get_mapped_range().expect("map").to_vec();
    Some(
        data.chunks_exact(4)
            .map(|chunk| i32::from_le_bytes(chunk.try_into().unwrap()))
            .collect(),
    )
}
