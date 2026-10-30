//! TT3 块纹理的 GPU **采样**（WGSL `sampler.wgsl`）：块内解码 + 双线性 + wrap。
//!
//! 与 [`crate::texture`] 的区别：那条把整图解成 RGBA；这条只解采样点附近的 4 个纹素，
//! 块流常驻显存。语义与 trit-texture 的 CPU 参考 `sample::sample_bilinear_with` 对齐
//! （纹素中心、clamp/repeat/mirror、f32 权重、round 后夹到 0..255），
//! 正确性由 `tests/texture_sample.rs` 逐字节对拍钉住。
//!
//! 没有 adapter 时返回 `None`（与 [`crate::gpu::run`] 同约定）。

use wgpu::util::DeviceExt;

const WORKGROUP: u32 = 64;
/// 每块字节数（与 trit-texture 的 `TRIT_BLOCK_BYTES` 对齐）。
const BLOCK_BYTES: usize = 12;
/// 块宽 / 块高（与 trit-texture 的 `BLOCK_WIDTH` / `BLOCK_HEIGHT` 对齐）。
const BLOCK_WIDTH: usize = 4;
const BLOCK_HEIGHT: usize = 10;

/// 越界寻址，数值与 WGSL 里的 `wrap` 参数一致。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wrap {
    /// 夹到边缘（0）。
    Clamp = 0,
    /// 平铺重复（1）。
    Repeat = 1,
    /// 镜像重复（2）。
    Mirror = 2,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    width: u32,
    height: u32,
    block_cols: u32,
    wrap: u32,
    count: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
}

/// 在 GPU 上按 `points`（每个 `(u, v)`）采样 TT3 纹理，返回 RGBA8（每样点 4 字节）。
///
/// `blocks` 是逐块拼接的字节流；`width` 须为 4 的倍数、`height` 须为 10 的倍数。
/// 形状或长度不符、或没有可用 adapter 时返回 `None`。
pub fn sample_tt3(
    blocks: &[u8],
    width: usize,
    height: usize,
    points: &[(f32, f32)],
    wrap: Wrap,
) -> Option<Vec<u8>> {
    pollster::block_on(sample_async(blocks, width, height, points, wrap))
}

async fn sample_async(
    blocks: &[u8],
    width: usize,
    height: usize,
    points: &[(f32, f32)],
    wrap: Wrap,
) -> Option<Vec<u8>> {
    if width == 0
        || !width.is_multiple_of(BLOCK_WIDTH)
        || height == 0
        || !height.is_multiple_of(BLOCK_HEIGHT)
    {
        return None;
    }
    let expected_blocks = (width / BLOCK_WIDTH) * (height / BLOCK_HEIGHT);
    if blocks.len() != expected_blocks * BLOCK_BYTES || points.is_empty() {
        return None;
    }

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .ok()?;
    let limits = adapter.limits();
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            required_limits: limits,
            ..Default::default()
        })
        .await
        .ok()?;

    let count = points.len() as u32;
    let blocks_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("tt3-sample-blocks"),
        contents: blocks,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let mut uv = Vec::with_capacity(points.len() * 2);
    for (u, v) in points {
        uv.push(*u);
        uv.push(*v);
    }
    let points_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("tt3-sample-points"),
        contents: bytemuck::cast_slice(&uv),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let out = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("tt3-sample-out"),
        size: u64::from(count) * 4,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let params = Params {
        width: width as u32,
        height: height as u32,
        block_cols: (width / BLOCK_WIDTH) as u32,
        wrap: wrap as u32,
        count,
        pad0: 0,
        pad1: 0,
        pad2: 0,
    };
    let params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("tt3-sample-params"),
        contents: bytemuck::bytes_of(&params),
        usage: wgpu::BufferUsages::UNIFORM,
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("tt3-sample"),
        source: wgpu::ShaderSource::Wgsl(include_str!("sampler.wgsl").into()),
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[
            storage_entry(0, true),
            storage_entry(1, true),
            storage_entry(2, false),
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("tt3-sample"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("main"),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: blocks_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: points_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: out.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: params_buf.as_entire_binding(),
            },
        ],
    });

    let groups = count.div_ceil(WORKGROUP);
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("tt3-sample"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(groups, 1, 1);
    }
    queue.submit([encoder.finish()]);

    let size = u64::from(count) * 4;
    let download = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("tt3-sample-readback"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    encoder.copy_buffer_to_buffer(&out, 0, &download, 0, size);
    queue.submit([encoder.finish()]);
    let slice = download.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
    let data = slice.get_mapped_range().ok()?;
    Some(bytemuck::pod_collect_to_vec(&data))
}

fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
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
