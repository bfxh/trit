// TTX2 块纹理：GPU 直采样 + 全图解码（与 trit-texture 的 CPU 解码逐字节对拍）。
//
// 块是 4 宽 × 8 高 = 32 纹素，有效内容 11 字节；上传前补齐到 12 字节（3 个 u32）：
//   word0: e0hi e0lo e1hi e1lo   （e0/e1 是 RGB565 的**大端** u16）
//   word1: 载荷字节 0..3
//   word2: 载荷字节 4..6 + 1 字节补零（任何分支都不读它）
// 载荷：32 纹素 × 1 trit，5 trit/字节，`Σ digit·3^slot`（低位在前）。
// 调色板 [e0, (e0+e1)/2, e1]，trit 0/1/2 依次取之；alpha 恒 255。
//
// 引擎直采样用法：绑定 blocks 后直接调 `ttx_sample_at(x, y, block_cols)`——
// 压缩块常驻显存、片元着色器现解码，不走「先展开成 RGBA 再采样」那条路
// （展开回 RGBA 就把省显存的意义抹掉了）。

struct Params {
    texels: u32,
    width: u32,
    block_cols: u32,
};

@group(0) @binding(0) var<storage, read> blocks: array<u32>;
@group(0) @binding(1) var<storage, read_write> out: array<u32>;
@group(0) @binding(2) var<uniform> params: Params;

fn expand5(v: u32) -> u32 {
    return (v << 3u) | (v >> 2u);
}

fn expand6(v: u32) -> u32 {
    return (v << 2u) | (v >> 4u);
}

// 3^slot（slot 0..4）；载荷按 3 进制取位，不是 2 进制位段。
fn pow3(slot: u32) -> u32 {
    var p = 1u;
    for (var i: u32 = 0u; i < slot; i = i + 1u) {
        p = p * 3u;
    }
    return p;
}

// 块内载荷第 k 字节（k 0..6）：k<4 落在 word1，k>=4 落在 word2。
fn payload_byte(base: u32, k: u32) -> u32 {
    let word = blocks[base + 1u + k / 4u];
    return (word >> ((k % 4u) * 8u)) & 0xffu;
}

// 块 base、块内序号 index（0..32）→ RGB。
fn ttx_texel_rgb(base: u32, index: u32) -> vec3<u32> {
    let word0 = blocks[base];
    let b0 = word0 & 0xffu;
    let b1 = (word0 >> 8u) & 0xffu;
    let b2 = (word0 >> 16u) & 0xffu;
    let b3 = (word0 >> 24u) & 0xffu;
    // 端点按大端组回 16 位——位序错一点全图花。
    let e0 = (b0 << 8u) | b1;
    let e1 = (b2 << 8u) | b3;
    let r0 = expand5((e0 >> 11u) & 0x1fu);
    let g0 = expand6((e0 >> 5u) & 0x3fu);
    let b0c = expand5(e0 & 0x1fu);
    let r1 = expand5((e1 >> 11u) & 0x1fu);
    let g1 = expand6((e1 >> 5u) & 0x3fu);
    let b1c = expand5(e1 & 0x1fu);
    let digit = (payload_byte(base, index / 5u) / pow3(index % 5u)) % 3u;
    if (digit == 2u) {
        return vec3<u32>(r1, g1, b1c);
    }
    if (digit == 1u) {
        return vec3<u32>((r0 + r1) / 2u, (g0 + g1) / 2u, (b0c + b1c) / 2u);
    }
    return vec3<u32>(r0, g0, b0c);
}

// 块网格行主序 → 块号：（y/8, x/4）；块内序号 = (y%8)*4 + (x%4)。
fn ttx_block_base(x: u32, y: u32, block_cols: u32) -> u32 {
    return ((y / 8u) * block_cols + (x / 4u)) * 3u;
}

// 直采样：按像素坐标取 0..1 的 RGBA，压缩块不展开。
fn ttx_sample_at(x: u32, y: u32, block_cols: u32) -> vec4<f32> {
    let index = (y % 8u) * 4u + (x % 4u);
    let rgb = ttx_texel_rgb(ttx_block_base(x, y, block_cols), index);
    return vec4<f32>(
        f32(rgb.x) / 255.0,
        f32(rgb.y) / 255.0,
        f32(rgb.z) / 255.0,
        1.0,
    );
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let texel = gid.x;
    if (texel >= params.texels) {
        return;
    }
    let x = texel % params.width;
    let y = texel / params.width;
    let index = (y % 8u) * 4u + (x % 4u);
    let rgb = ttx_texel_rgb(ttx_block_base(x, y, params.block_cols), index);
    out[texel] = rgb.x | (rgb.y << 8u) | (rgb.z << 16u) | (255u << 24u);
}
