//! 计时与回读的共用小工具（仅 GPU feature）。

use std::time::Instant;
pub(crate) fn median_ns_per(device: &wgpu::Device, work: usize, mut submit: impl FnMut()) -> f64 {
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
        samples.push(start.elapsed().as_nanos() as f64 / (iters as f64 * work as f64));
    }
    samples.sort_by(|a, b| a.partial_cmp(b).expect("有限值"));
    samples[samples.len() / 2]
}

pub(crate) fn read_u32s(
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

// 与 wgpu 绑定布局相关的共用 helper（从 matmul.rs 迁入，避免单文件超 800 行）。

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
