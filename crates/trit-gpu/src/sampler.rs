//! TT3 块纹理的 GPU **采样**（WGSL `sampler.wgsl`）：mip 链 + 双线性 + wrap。
//!
//! 与 [`crate::texture`] 的区别：那条把整图解成 RGBA；这条只解采样点附近的 4 个纹素，
//! 块流常驻显存。语义与 trit-texture 的 CPU 参考 `sample::sample_bilinear_with` /
//! `TtxDocument::sample_trilinear_with` 对齐（纹素中心、clamp/repeat/mirror、
//! 显式 fma、round ties-to-even），正确性由 `tests/texture_sample.rs` 对拍钉住。
//!
//! 两种用法：
//! - [`GpuSampler`]：**建一次设备 + 管线**，之后 `sample()` 只重传采样点并回读——
//!   引擎/批处理该走这条（实测一次性的设备+管线开销约 0.4–1.8 s，会把小批量完全吃掉）。
//! - [`sample_tt3_mips`]：一次性包装，内部就是 `GpuSampler::new(...).sample(...)`。
//!
//! 实测（本机 RTX 4060 Ti，2048×2040 + 9 级 mip，4.18M 样点，5–7 轮中位数）：
//!
//! | 路径 | ns/样点 |
//! |---|---:|
//! | CPU 参考（`sample_trilinear_with`） | 70.1 |
//! | GPU 持久采样器 | **11.0**（6.4×） |
//! | GPU 一次性调用（每次重建设备） | 222.6（init ≈ 1.6 s 吃掉全部） |
//!
//! 常驻块流 1633 KiB vs RGBA 16320 KiB（**10.0×**）。所以收益成立的前提是
//! **复用设备**——一次性调用比 CPU 还慢 3 倍，别那样用。
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

/// 各向异性采样点：`taps` 个三线性点沿 `(step_u, step_v)` 在半像素跨度内均匀取。
#[derive(Clone, Copy, Debug)]
pub struct AnisoSample {
    /// u（`[0,1]`，越界按 wrap 处理）。
    pub u: f32,
    /// v。
    pub v: f32,
    /// 三线性 LOD。
    pub lod: f32,
    /// 采样点数（≥1；1 = 退化为一次三线性）。
    pub taps: u32,
    /// 单像素在 u 上的跨度（长轴方向）。
    pub step_u: f32,
    /// 单像素在 v 上的跨度（长轴方向）。
    pub step_v: f32,
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

/// 持久 GPU 采样器：设备、管线、块流只建一次，之后每次 `sample()` 只重传采样点。
///
/// ```no_run
/// # use trit_gpu::sampler::{GpuSampler, MipLevel, Wrap};
/// # let (blocks, w, h) = (vec![0u8; 0], 4, 10);
/// let mut sampler = GpuSampler::new(&[MipLevel { blocks: &blocks, width: w, height: h, rect: None }], Wrap::Clamp);
/// ```
pub struct GpuSampler {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    bind_layout: wgpu::BindGroupLayout,
    blocks_buf: wgpu::Buffer,
    levels_buf: wgpu::Buffer,
    level_count: u32,
    wrap: Wrap,
    points_buf: Option<wgpu::Buffer>,
    points_cap: usize,
    out_buf: Option<wgpu::Buffer>,
    out_cap: usize,
    download: Option<wgpu::Buffer>,
}

impl GpuSampler {
    /// 建设备 + 管线 + 上传块流（一次）。形状/长度不符或没有 adapter 返回 `None`。
    pub fn new(levels: &[MipLevel<'_>], wrap: Wrap) -> Option<Self> {
        pollster::block_on(Self::new_async(levels, wrap))
    }

    async fn new_async(levels: &[MipLevel<'_>], wrap: Wrap) -> Option<Self> {
        if levels.is_empty() {
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
            let rect_bad = matches!(
                level.rect,
                Some(r) if r.width == 0
                    || r.height == 0
                    || r.x + r.width > level.width
                    || r.y + r.height > level.height
            );
            if rect_bad {
                return None;
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
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("tt3-sample"),
            source: wgpu::ShaderSource::Wgsl(
                concat!(
                    include_str!("sampler_common.wgsl"),
                    include_str!("sampler_entry.wgsl")
                )
                .into(),
            ),
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                // 0/1/2 = 块流 / 各级描述符 / Params（与光栅化片元共用同一份 WGSL）
                storage_entry(0, true),
                storage_entry(1, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                storage_entry(3, true),
                storage_entry(4, false),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&bind_layout)],
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
        Some(Self {
            device,
            queue,
            pipeline,
            bind_layout,
            blocks_buf,
            levels_buf,
            level_count: levels.len() as u32,
            wrap,
            points_buf: None,
            points_cap: 0,
            out_buf: None,
            out_cap: 0,
            download: None,
        })
    }

    /// 级数。
    pub fn level_count(&self) -> u32 {
        self.level_count
    }

    /// 三线性采样一批 `(u, v, lod)`，返回 RGBA8（每样点 4 字节）。
    pub fn sample(&mut self, points: &[(f32, f32, f32)]) -> Option<Vec<u8>> {
        let raw: Vec<[f32; 8]> = points
            .iter()
            .map(|(u, v, lod)| [*u, *v, *lod, 1.0, 0.0, 0.0, 0.0, 0.0])
            .collect();
        self.submit(&raw)
    }

    /// 各向异性采样一批 [`AnisoSample`]（每点 `taps` 个三线性点平均）。
    pub fn sample_aniso(&mut self, samples: &[AnisoSample]) -> Option<Vec<u8>> {
        let raw: Vec<[f32; 8]> = samples
            .iter()
            .map(|s| {
                [
                    s.u,
                    s.v,
                    s.lod,
                    s.taps.max(1) as f32,
                    s.step_u,
                    s.step_v,
                    0.0,
                    0.0,
                ]
            })
            .collect();
        self.submit(&raw)
    }

    /// 每样点 32 字节：`(u, v, lod, taps) + (step_u, step_v, _, _)`。
    fn submit(&mut self, raw: &[[f32; 8]]) -> Option<Vec<u8>> {
        if raw.is_empty() {
            return None;
        }
        let count = raw.len();
        if self.points_cap < count {
            self.points_buf = Some(self.device.create_buffer_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some("tt3-sample-points"),
                    contents: bytemuck::cast_slice(raw),
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                },
            ));
            self.points_cap = count;
        } else {
            self.queue
                .write_buffer(self.points_buf.as_ref()?, 0, bytemuck::cast_slice(raw));
        }
        let size = (count * 4) as u64;
        if self.out_cap < count {
            let out = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("tt3-sample-out"),
                size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let download = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("tt3-sample-readback"),
                size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            self.out_buf = Some(out);
            self.download = Some(download);
            self.out_cap = count;
        }
        let params = Params {
            level_count: self.level_count,
            wrap: self.wrap as u32,
            count: count as u32,
            pad0: 0,
        };
        let params_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("tt3-sample-params"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.blocks_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.levels_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.points_buf.as_ref()?.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: self.out_buf.as_ref()?.as_entire_binding(),
                },
            ],
        });

        let groups = (count as u32).div_ceil(WORKGROUP);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("tt3-sample"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(groups, 1, 1);
        }
        let out_buf = self.out_buf.as_ref()?;
        let download = self.download.as_ref()?;
        encoder.copy_buffer_to_buffer(out_buf, 0, download, 0, size);
        self.queue.submit([encoder.finish()]);
        let slice = download.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        let data = slice.get_mapped_range().ok()?;
        let bytes = bytemuck::pod_collect_to_vec(&data);
        drop(data);
        download.unmap();
        Some(bytes)
    }
}

/// 一次性三线性采样：内部就是 `GpuSampler::new(...).sample(...)`。
///
/// 每次调用都会重建设备与管线（0.4–1.8 s 量级）；批量/引擎请直接用 [`GpuSampler`]。
pub fn sample_tt3_mips(
    levels: &[MipLevel<'_>],
    points: &[(f32, f32, f32)],
    wrap: Wrap,
) -> Option<Vec<u8>> {
    let mut sampler = GpuSampler::new(levels, wrap)?;
    sampler.sample(points)
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
