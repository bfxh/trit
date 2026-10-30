//! GPU 光栅管线：顶点透传 → 光栅化 → 片元自己选 LOD 并三线性采样 → 深度测试 → RGB8 回读。
//!
//! 与 [`crate::sampler`] 的区别：那条是**逐点**内核，采样点和 LOD 得由 CPU 算好再上传；
//! 这条是真正的绘制管线——顶点、覆盖、透视插值、深度、导数全在 GPU 上，采样点常驻显存。
//!
//! 与 CPU 参考（`trit_texture::raster`，dev-dependency）的差别只有一处：
//! GPU 的导数是 2×2 quad 的 `dpdx`/`dpdy` 有限差分，CPU 是解析式。所以
//! - `force_lod = Some(lod)`：两边采样参数完全相同 ⇒ 判据是有界逐像素相等；
//! - `force_lod = None`：LOD 来源不同 ⇒ 只做有界对拍（且要能区分 mip 是否生效）。
//!
//! 顶点直接给**裁剪空间**（与 CPU 参考一致），不做近/远平面裁剪；`w ≤ 0` 的三角形
//! 由 GPU 自己裁掉。深度是 `[0, 1]` 的 NDC z，`Less` 测试，越小越近。

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::sampler::{MipLevel, Wrap};

const BLOCK_BYTES: usize = 12;
const BLOCK_WIDTH: usize = 4;
const BLOCK_HEIGHT: usize = 10;

/// 顶点：裁剪空间位置 + UV（与 CPU 参考同构）。
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Vertex {
    /// 裁剪空间 `(x, y, z, w)`。
    pub clip: [f32; 4],
    /// 纹理坐标。
    pub uv: [f32; 2],
}

/// 三角网格：顶点表 + `u32` 索引（每项 3 个）。
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    /// 顶点。
    pub vertices: Vec<Vertex>,
    /// 三角形。
    pub indices: Vec<[u32; 3]>,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    level_count: u32,
    wrap: u32,
    count: u32,
    pad0: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RenderParams {
    use_force_lod: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
    force_lod: f32,
    pad3: f32,
    pad4: f32,
    pad5: f32,
}

/// GPU 光栅化器：设备、管线、块流都在 [`GpuRenderer::new`] 里建一次。
pub struct GpuRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    bind_layout: wgpu::BindGroupLayout,
    blocks_buf: wgpu::Buffer,
    levels_buf: wgpu::Buffer,
    params_buf: wgpu::Buffer,
    render_buf: wgpu::Buffer,
    level_count: u32,
    wrap: Wrap,
}

impl GpuRenderer {
    /// 建设备 + 管线 + 上传块流（一次）。形状不符或没有 adapter 返回 `None`。
    pub fn new(levels: &[MipLevel<'_>], wrap: Wrap) -> Option<Self> {
        pollster::block_on(Self::new_async(levels, wrap))
    }

    async fn new_async(levels: &[MipLevel<'_>], wrap: Wrap) -> Option<Self> {
        let (packed, descriptors) = pack_levels(levels)?;
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
            label: Some("tt3-raster-blocks"),
            contents: &packed,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let levels_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("tt3-raster-levels"),
            contents: bytemuck::cast_slice(&descriptors),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let params = Params {
            level_count: levels.len() as u32,
            wrap: wrap as u32,
            count: 0,
            pad0: 0,
        };
        let params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("tt3-raster-params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let render_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("tt3-raster-render-params"),
            contents: bytemuck::bytes_of(&RenderParams {
                use_force_lod: 0,
                pad0: 0,
                pad1: 0,
                pad2: 0,
                force_lod: 0.0,
                pad3: 0.0,
                pad4: 0.0,
                pad5: 0.0,
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("tt3-raster"),
            source: wgpu::ShaderSource::Wgsl(
                concat!(
                    include_str!("sampler_common.wgsl"),
                    include_str!("raster_entry.wgsl")
                )
                .into(),
            ),
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                uniform_or_storage(0, true),
                uniform_or_storage(1, true),
                uniform_or_storage(2, false),
                uniform_or_storage(3, false),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("tt3-raster"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 16,
                            shader_location: 1,
                        },
                    ],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Some(Self {
            device,
            queue,
            pipeline,
            bind_layout,
            blocks_buf,
            levels_buf,
            params_buf,
            render_buf,
            level_count: levels.len() as u32,
            wrap,
        })
    }

    /// 当前越界寻址。
    pub fn wrap(&self) -> Wrap {
        self.wrap
    }

    /// 级数。
    pub fn level_count(&self) -> u32 {
        self.level_count
    }

    /// 渲染一帧，返回行优先 RGB8（`width * height * 3`）。
    ///
    /// `force_lod`：`Some(lod)` 强制三线性 LOD（用于与 CPU 参考逐像素对拍）；
    /// `None` 则片元里用 `dpdx`/`dpdy` 自动选。
    pub fn render(
        &self,
        mesh: &Mesh,
        width: u32,
        height: u32,
        clear: [u8; 3],
        force_lod: Option<f32>,
    ) -> Option<Vec<u8>> {
        if width == 0 || height == 0 || mesh.vertices.is_empty() || mesh.indices.is_empty() {
            return None;
        }
        if mesh
            .indices
            .iter()
            .flatten()
            .any(|&i| i as usize >= mesh.vertices.len())
        {
            return None;
        }

        let params = RenderParams {
            use_force_lod: u32::from(force_lod.is_some()),
            pad0: 0,
            pad1: 0,
            pad2: 0,
            force_lod: force_lod.unwrap_or(0.0),
            pad3: 0.0,
            pad4: 0.0,
            pad5: 0.0,
        };
        self.queue
            .write_buffer(&self.render_buf, 0, bytemuck::bytes_of(&params));

        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("tt3-raster-target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let depth = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("tt3-raster-depth"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let color_view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());

        let vertex_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("tt3-raster-vertices"),
                contents: bytemuck::cast_slice(&mesh.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let index_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("tt3-raster-indices"),
                contents: bytemuck::cast_slice(&mesh.indices),
                usage: wgpu::BufferUsages::INDEX,
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
                    resource: self.params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.render_buf.as_entire_binding(),
                },
            ],
        });

        // 行对齐 256 字节（`copy_texture_to_buffer` 的硬要求）
        let unpadded = width as usize * 4;
        let padded = unpadded.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("tt3-raster-readback"),
            size: (padded * height as usize) as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("tt3-raster"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: clear[0] as f64 / 255.0,
                            g: clear[1] as f64 / 255.0,
                            b: clear[2] as f64 / 255.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.set_vertex_buffer(0, vertex_buf.slice(..));
            pass.set_index_buffer(index_buf.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.indices.len() as u32 * 3, 0, 0..1);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded as u32),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);

        let slice = staging.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        let data = slice.get_mapped_range().ok()?;
        let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
        for row in data.chunks_exact(padded) {
            for px in row[..unpadded].as_chunks::<4>().0 {
                rgb.extend_from_slice(&px[..3]);
            }
        }
        drop(data);
        staging.unmap();
        Some(rgb)
    }
}

/// 校验形状并把各级块流拼成一份 `u32` 数组 + 描述符表。
fn pack_levels(levels: &[MipLevel<'_>]) -> Option<(Vec<u8>, Vec<[u32; 8]>)> {
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
        if let Some(r) = level.rect
            && (r.width == 0
                || r.height == 0
                || r.x + r.width > level.width
                || r.y + r.height > level.height)
        {
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
    Some((packed, descriptors))
}

/// 绑定项：`storage=true` 是只读存储，否则是 uniform；两种都只给片元阶段用。
fn uniform_or_storage(binding: u32, storage: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: if storage {
                wgpu::BufferBindingType::Storage { read_only: true }
            } else {
                wgpu::BufferBindingType::Uniform
            },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}
