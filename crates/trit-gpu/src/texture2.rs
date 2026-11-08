//! TTX2 块纹理的 GPU 直采样 / 全图解码（WGSL `texture2.wgsl`）。
//!
//! 与 [`crate::texture`] 的差别：那条是 **4×10、12 字节** 的老 TT3 布局，与
//! 现行 `trit-texture`（4×8、11 字节）不符——传真实块流会在长度校验上失败并
//! 静默返回 `None`，所以那条路径从未被真正对拍过。这条才是匹配现行编码器的实现。
//!
//! 格式契约在 trit-texture（跨实验层不能互相依赖，见发布图门），这里只接收
//! **扁平的块字节流**（每块 11 字节，宽为 4 的倍数、高为 8 的倍数）；正确性由
//! `tests/texture2_decode.rs` 用 trit-texture 编码 + CPU 解码逐字节对拍钉住。
//! 没有 adapter 时 GPU 路径返回 `None`（与 [`crate::gpu::run`] 同约定）。
//!
//! 引擎侧要省显存就调 [`TTX2_WGSL`] 里的 `ttx_sample_at`：压缩块常驻显存、
//! 片元着色器现解码。走 [`decode_ttx2_blocks`] 整图解成 RGBA 只是验证/回退用，
//! 那一步会把省下来的显存又吃回去。

use wgpu::util::DeviceExt;

const WORKGROUP: u32 = 64;
/// 块宽（与 trit-texture 的 `BLOCK_WIDTH` 对齐）。
pub const BLOCK_WIDTH: usize = 4;
/// 块高（与 trit-texture 的 `BLOCK_HEIGHT` 对齐）。
pub const BLOCK_HEIGHT: usize = 8;
/// 每块有效字节数：4 字节 RGB565 端点 + 7 字节 trit 载荷。
pub const BLOCK_BYTES: usize = 11;
/// 上传对齐后的每块字节数（补 1 字节，凑 3 个 u32，GPU 才好按字取）。
pub const PADDED_BLOCK_BYTES: usize = 12;

/// 直采样着色器源码（含 `ttx_sample_at`），供引擎 include 进自己的管线。
pub const TTX2_WGSL: &str = include_str!("texture2.wgsl");

/// 形状与长度校验；返回块列数、块行数。
fn geometry(blocks: &[u8], width: usize, height: usize) -> Option<(usize, usize)> {
    if width == 0 || !width.is_multiple_of(BLOCK_WIDTH) {
        return None;
    }
    if height == 0 || !height.is_multiple_of(BLOCK_HEIGHT) {
        return None;
    }
    let block_cols = width / BLOCK_WIDTH;
    let block_rows = height / BLOCK_HEIGHT;
    if blocks.len() != block_cols * block_rows * BLOCK_BYTES {
        return None;
    }
    Some((block_cols, block_rows))
}

/// 把 11 字节/块的流补齐成 12 字节/块的 u32 流——直采样上传就传这个。
///
/// 补齐后每块 12 字节 / 32 纹素 = 0.375 B/纹素，对 RGBA8（4 B/纹素）是
/// **10.67×**；补的那 1 字节不被任何分支读取。
///
/// 长度不是 11 的整数倍时**尾部不足一块的部分被丢弃**（调用方应先过
/// [`decode_ttx2_blocks`] / [`decode_ttx2_blocks_cpu`] 的长度校验）。
pub fn padded_words(blocks: &[u8]) -> Vec<u32> {
    let count = blocks.len() / BLOCK_BYTES;
    let mut out = Vec::with_capacity(count * 3);
    for index in 0..count {
        let start = index * BLOCK_BYTES;
        let mut padded = [0u8; PADDED_BLOCK_BYTES];
        padded[..BLOCK_BYTES].copy_from_slice(&blocks[start..start + BLOCK_BYTES]);
        out.push(u32::from_le_bytes([
            padded[0], padded[1], padded[2], padded[3],
        ]));
        out.push(u32::from_le_bytes([
            padded[4], padded[5], padded[6], padded[7],
        ]));
        out.push(u32::from_le_bytes([
            padded[8], padded[9], padded[10], padded[11],
        ]));
    }
    out
}

fn expand5(v: u32) -> u32 {
    (v << 3) | (v >> 2)
}

fn expand6(v: u32) -> u32 {
    (v << 2) | (v >> 4)
}

/// 块内载荷第 k 字节（k 0..6）：k<4 落在 word1，k>=4 落在 word2。
fn payload_byte(words: &[u32], base: usize, k: usize) -> u32 {
    let word = words[base + 1 + k / 4];
    (word >> ((k % 4) * 8)) & 0xff
}

/// 块 `base`、块内序号 `index`（0..32）→ RGB；与 WGSL 的 `ttx_texel_rgb` 同算法。
fn texel_rgb(words: &[u32], base: usize, index: usize) -> (u8, u8, u8) {
    let word0 = words[base];
    let b0 = word0 & 0xff;
    let b1 = (word0 >> 8) & 0xff;
    let b2 = (word0 >> 16) & 0xff;
    let b3 = (word0 >> 24) & 0xff;
    // 端点按大端组回 16 位——位序错一点全图花。
    let e0 = (b0 << 8) | b1;
    let e1 = (b2 << 8) | b3;
    let r0 = expand5((e0 >> 11) & 0x1f);
    let g0 = expand6((e0 >> 5) & 0x3f);
    let b0c = expand5(e0 & 0x1f);
    let r1 = expand5((e1 >> 11) & 0x1f);
    let g1 = expand6((e1 >> 5) & 0x3f);
    let b1c = expand5(e1 & 0x1f);
    // 3 进制取位：digit = (byte / 3^slot) % 3，不是 2 进制位段。
    let digit = (payload_byte(words, base, index / 5) / 3u32.pow((index % 5) as u32)) % 3;
    match digit {
        2 => (r1 as u8, g1 as u8, b1c as u8),
        1 => (
            ((r0 + r1) / 2) as u8,
            ((g0 + g1) / 2) as u8,
            ((b0c + b1c) / 2) as u8,
        ),
        _ => (r0 as u8, g0 as u8, b0c as u8),
    }
}

/// 块网格行主序 → 块基址（单位：u32 字）。
fn block_base(x: usize, y: usize, block_cols: usize) -> usize {
    ((y / BLOCK_HEIGHT) * block_cols + (x / BLOCK_WIDTH)) * 3
}

/// 与 `texture2.wgsl` **同一套算法**的 CPU 镜像。
///
/// 存在意义：没有 GPU adapter 时也能钉住「块布局 + 3 进制取位 + 调色板」的正确性，
/// 不至于让 GPU 测试静默跳过就当过了。
pub fn decode_ttx2_blocks_cpu(blocks: &[u8], width: usize, height: usize) -> Option<Vec<u8>> {
    let (block_cols, _) = geometry(blocks, width, height)?;
    let words = padded_words(blocks);
    let mut out = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let index = (y % BLOCK_HEIGHT) * BLOCK_WIDTH + (x % BLOCK_WIDTH);
            let (r, g, b) = texel_rgb(&words, block_base(x, y, block_cols), index);
            let offset = (y * width + x) * 4;
            out[offset] = r;
            out[offset + 1] = g;
            out[offset + 2] = b;
            out[offset + 3] = 255;
        }
    }
    Some(out)
}

/// 用 GPU 解码一整张 TTX2 纹理；返回 RGBA（每纹素 4 字节，alpha 255）。
///
/// `blocks` 是逐块拼接的字节流（每块 11 字节），`width` 须为 4 的倍数、
/// `height` 须为 8 的倍数；形状或长度不符返回 `None`。
pub fn decode_ttx2_blocks(blocks: &[u8], width: usize, height: usize) -> Option<Vec<u8>> {
    pollster::block_on(decode_async(blocks, width, height))
}

async fn decode_async(blocks: &[u8], width: usize, height: usize) -> Option<Vec<u8>> {
    let (block_cols, _) = geometry(blocks, width, height)?;
    let words = padded_words(blocks);

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
    let blocks_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("ttx2-blocks"),
        contents: bytemuck::cast_slice(&words),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let out = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ttx2-rgba"),
        size: u64::from(texels) * 4,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("ttx2-params"),
        contents: bytemuck::cast_slice(&[texels, width as u32, block_cols as u32]),
        usage: wgpu::BufferUsages::UNIFORM,
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("ttx2-decode"),
        source: wgpu::ShaderSource::Wgsl(TTX2_WGSL.into()),
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
        label: Some("ttx2-decode"),
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
        label: Some("ttx2-staging"),
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
