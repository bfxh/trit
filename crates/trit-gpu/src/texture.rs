//! TT3 块纹理的 GPU 解码（WGSL `texture.wgsl`）。
//!
//! 格式契约在 trit-texture（跨实验层不能互相依赖，见发布图门），这里只接收
//! **扁平的块字节流**（每块 17 字节 = 4 端点 + 13 载荷，宽高皆 8 的倍数）；正确性由
//! `tests/texture_decode.rs` 用 trit-texture 编码 + CPU 解码逐字节对拍钉住。
//! 没有 adapter 时返回 `None`（与 [`crate::gpu::run`] 同约定）。

use wgpu::util::DeviceExt;

const WORKGROUP: u32 = 64;
/// 每块字节数（与 trit-texture 的 TRIT_BLOCK_BYTES 对齐）。
const BLOCK_BYTES: usize = 17;
/// 块边长（与 trit-texture 的 BLOCK_WIDTH/BLOCK_HEIGHT 对齐）。
const BLOCK_SIDE: usize = 8;

/// 用 GPU 解码一整张 TT3 纹理；返回 RGBA（每纹素 4 字节，alpha 255）。
///
/// `blocks` 是逐块拼接的字节流，`width`/`height` 须为 8 的倍数；
/// 形状或长度不符返回 `None`。
pub fn decode_tt3_blocks(blocks: &[u8], width: usize, height: usize) -> Option<Vec<u8>> {
    pollster::block_on(decode_async(blocks, width, height))
}

async fn decode_async(blocks: &[u8], width: usize, height: usize) -> Option<Vec<u8>> {
    if width == 0
        || !width.is_multiple_of(BLOCK_SIDE)
        || height == 0
        || !height.is_multiple_of(BLOCK_SIDE)
    {
        return None;
    }
    let expected_blocks = (width / BLOCK_SIDE) * (height / BLOCK_SIDE);
    if blocks.len() != expected_blocks * BLOCK_BYTES {
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

    let texels = (width * height) as u32;
    // 17 字节/块不是 4 字节的倍数：块起点不再 u32 对齐，着色器按**全局字节偏移**
    // 取字节（见 texture.wgsl）。上传前把字节流补齐到 4 字节倍数，只补尾部，
    // 不改变任何块的偏移。
    let mut padded = blocks.to_vec();
    while !padded.len().is_multiple_of(4) {
        padded.push(0);
    }
    let words: &[u32] = bytemuck::cast_slice(&padded);
    let blocks_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("tt3-blocks"),
        contents: bytemuck::cast_slice(words),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let out = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("tt3-rgba"),
        size: u64::from(texels) * 4,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("tt3-params"),
        contents: bytemuck::cast_slice(&[
            texels,
            width as u32,
            (width / BLOCK_SIDE) as u32,
            BLOCK_BYTES as u32,
        ]),
        usage: wgpu::BufferUsages::UNIFORM,
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("tt3-decode"),
        source: wgpu::ShaderSource::Wgsl(include_str!("texture.wgsl").into()),
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
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
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("tt3-decode"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: blocks_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: out.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: params.as_entire_binding(),
            },
        ],
    });

    let grid = texels.div_ceil(WORKGROUP);
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(grid, 1, 1);
    }
    queue.submit([encoder.finish()]);

    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("tt3-staging"),
        size: u64::from(texels) * 4,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut copy = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    copy.copy_buffer_to_buffer(&out, 0, &staging, 0, u64::from(texels) * 4);
    queue.submit([copy.finish()]);
    let slice = staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let mapped = slice.get_mapped_range().expect("map");
    Some(mapped.to_vec())
}
