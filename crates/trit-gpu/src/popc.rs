//! countOneBits 吞吐微基准：判断矩阵乘离 popc 天花板还剩多少空间。

use std::time::Instant;

use wgpu::util::DeviceExt;

use crate::bench::{storage_with, uniform};

pub(crate) fn ceiling(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    shader: &wgpu::ShaderModule,
) -> Option<f64> {
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
        module: shader,
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
    Some(popc_ops / popc_seconds)
}
