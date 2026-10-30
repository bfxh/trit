//! TT3 块纹理的 GPU **采样**（WGSL `sampler.wgsl`）：mip 链 + 双线性 + wrap。
//!
//! 与 [`crate::texture`] 的区别：那条把整图解成 RGBA；这条只解采样点附近的 4 个纹素，
//! 块流常驻显存。语义与 trit-texture 的 CPU 参考 `sample::sample_bilinear_with` /
//! `TtxDocument::sample_trilinear_with` 对齐（纹素中心、clamp/repeat/mirror、
//! 显式 fma、round ties-to-even），正确性由 `tests/texture_sample.rs` 对拍钉住。
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
    level_count: u32,
    wrap: u32,
    count: u32,
    pad0: u32,
}

/// 图集子矩形（该级纹素坐标）。`None` = 整级。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    /// 左边界。
    pub x: usize,
    /// 上边界。
    pub y: usize,
    /// 宽（> 0）。
    pub width: usize,
    /// 高（> 0）。
    pub height: usize,
}

/// 一级 mip：块流 + 尺寸（+ 可选图集子矩形）。
#[derive(Clone, Copy, Debug)]
pub struct MipLevel<'a> {
    /// 逐块拼接的块字节流（每块 12 字节）。
    pub blocks: &'a [u8],
    /// 宽（须为 4 的倍数）。
    pub width: usize,
    /// 高（须为 10 的倍数）。
    pub height: usize,
    /// 只采这一块子矩形（纹素坐标，**按级给**，调用方自己按 2^level 缩放）；`None` = 整级。
    pub rect: Option<Rect>,
}

/// 单级采样（`lod = 0`）：等价于 [`sample_tt3_mips`] 只给一级。
pub fn sample_tt3(
    blocks: &[u8],
    width: usize,
    height: usize,
    points: &[(f32, f32)],
    wrap: Wrap,
) -> Option<Vec<u8>> {
    let pts: Vec<(f32, f32, f32)> = points.iter().map(|(u, v)| (*u, *v, 0.0)).collect();
    sample_tt3_mips(
        &[MipLevel {
            blocks,
            width,
            height,
            rect: None,
        }],
        &pts,
        wrap,
    )
}

/// 在 GPU 上按 `(u, v, lod)` **三线性**采样 mip 链，返回 RGBA8（每样点 4 字节）。
///
/// 各级块流按顺序拼进一个缓冲，级内偏移由 host 算好；`lod` 夹到 `[0, 级数-1]`、
/// NaN 按 0 处理（与 CPU 同口径）。形状或长度不符、或没有可用 adapter 时返回 `None`。
pub fn sample_tt3_mips(
    levels: &[MipLevel<'_>],
    points: &[(f32, f32, f32)],
    wrap: Wrap,
) -> Option<Vec<u8>> {
    pollster::block_on(sample_async(levels, points, wrap))
}

async fn sample_async(
    levels: &[MipLevel<'_>],
    points: &[(f32, f32, f32)],
    wrap: Wrap,
) -> Option<Vec<u8>> {
    if levels.is_empty() || points.is_empty() {
        return None;
    }
    let mut descriptors: Vec<[u32; 8]> = Vec::with_capacity(levels.len());
    let mut packed: Vec<u8> = Vec::new();
    for level in levels {
        if level.width == 0
            || !level.width.is_multiple_of(BLOCK_WIDTH)
            || level.height == 0
            || !level.height.is_multiple_of(BLOCK_HEIGHT)
        {
            return None;
        }
        let blocks = (level.width / BLOCK_WIDTH) * (level.height / BLOCK_HEIGHT);
        if level.blocks.len() != blocks * BLOCK_BYTES {
            return None;
        }
        if let Some(r) = level.rect {
            if r.width == 0
                || r.height == 0
                || r.x + r.width > level.width
                || r.y + r.height > level.height
            {
                return None;
            }
        }
        let rect = level.rect;
        descriptors.push([
            (packed.len() / BLOCK_BYTES) as u32,
            level.width as u32,
            level.height as u32,
            (level.width / BLOCK_WIDTH) as u32,
            rect.map_or(0, |r| r.x as u32),
            rect.map_or(0, |r| r.y as u32),
            rect.map_or(0, |r| r.width as u32),
            rect.map_or(0, |r| r.height as u32),
        ]);
        packed.extend_from_slice(level.blocks);
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
        contents: &packed,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let levels_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("tt3-sample-levels"),
        contents: bytemuck::cast_slice(&descriptors),
        usage: wgpu::BufferUsages::STORAGE,
    });
    // WGSL 里是 array<vec3<f32>>：**stride 16**，所以每点补一个 pad 分量
    let mut uv: Vec<f32> = Vec::with_capacity(points.len() * 4);
    for (u, v, lod) in points {
        uv.extend_from_slice(&[*u, *v, *lod, 0.0]);
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
        level_count: levels.len() as u32,
        wrap: wrap as u32,
        count,
        pad0: 0,
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
            storage_entry(2, true),
            storage_entry(3, false),
            wgpu::BindGroupLayoutEntry {
                binding: 4,
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
                resource: levels_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: points_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: out.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
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
