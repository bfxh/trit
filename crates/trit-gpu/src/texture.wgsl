// TT3 块纹理解码（与 trit-texture 的 CPU 解码逐字节对拍）。
//
// 块布局（v2：8×8 = 64 纹素，每块 17 字节）：
//   字节 0-1: e0（RGB565，大端）
//   字节 2-3: e1（RGB565，大端）
//   字节 4-16: 64 纹素 × 1 trit，5 trit/字节（基 3 打包，低位在前）
//
// ⚠ 17 字节**不是 4 的倍数**，块不再按 u32 对齐。旧实现按 `block_id * 3u` 取字，
// 依赖 12 字节/块的整除关系；现在必须按**全局字节偏移**寻址。宿主上传前把字节流
// 补齐到 4 字节倍数（只补尾，不改变格式与块偏移），这里再按偏移拆字取字节。
//
// 一线程一纹素；调色板 {e0, mid, e1} 的展开与 (a+b)/2 中点必须与 CPU 完全一致。

struct Params {
    texels: u32,
    width: u32,
    block_cols: u32,
    block_bytes: u32,
};

// 尾字节可能被补齐到 4 的倍数，声明为运行期长度数组以免越界校验误判。
@group(0) @binding(0) var<storage, read> blocks: array<u32>;
@group(0) @binding(1) var<storage, read_write> out: array<u32>;
@group(0) @binding(2) var<uniform> params: Params;

fn expand5(v: u32) -> u32 {
    return (v << 3u) | (v >> 2u);
}

fn expand6(v: u32) -> u32 {
    return (v << 2u) | (v >> 4u);
}

// 按**全局字节偏移**取一个字节（块不再 u32 对齐，不能再用块内字下标）。
fn byte_at(offset: u32) -> u32 {
    let word = blocks[offset / 4u];
    return (word >> ((offset % 4u) * 8u)) & 0xffu;
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let texel = gid.x;
    if (texel >= params.texels) {
        return;
    }
    // 输出像素 (x, y) → 块 (x/8, y/8) → 块内序 index = (y%8)*8 + x%8
    //（与编码器 index%8 = x 偏移、index/8 = y 偏移一致）。
    let x = texel % params.width;
    let y = texel / params.width;
    let block_col = x / 8u;
    let block_row = y / 8u;
    let block_id = block_row * params.block_cols + block_col;
    let index = (y % 8u) * 8u + (x % 8u);

    let base = block_id * params.block_bytes;
    // 端点按 BE 组回 16 位：字节序错一点全图花。
    let e0 = (byte_at(base) << 8u) | byte_at(base + 1u);
    let e1 = (byte_at(base + 2u) << 8u) | byte_at(base + 3u);
    let r0 = expand5((e0 >> 11u) & 0x1fu);
    let g0 = expand6((e0 >> 5u) & 0x3fu);
    let b0c = expand5(e0 & 0x1fu);
    let r1 = expand5((e1 >> 11u) & 0x1fu);
    let g1 = expand6((e1 >> 5u) & 0x3fu);
    let b1c = expand5(e1 & 0x1fu);

    // 5 trit/字节是**基 3** 打包（3^5 = 243 ≤ 256），不是 3-bit 位段：
    // 第 i 个 trit = byte / 3^i % 3。
    let k = index / 5u;
    var pow3 = 1u;
    for (var i = 0u; i < index % 5u; i = i + 1u) {
        pow3 = pow3 * 3u;
    }
    let trit = (byte_at(base + 4u + k) / pow3) % 3u;

    var r = 0u;
    var g = 0u;
    var b = 0u;
    if (trit == 0u) {
        r = r0;
        g = g0;
        b = b0c;
    } else if (trit == 1u) {
        r = (r0 + r1) / 2u;
        g = (g0 + g1) / 2u;
        b = (b0c + b1c) / 2u;
    } else {
        r = r1;
        g = g1;
        b = b1c;
    }
    out[texel] = r | (g << 8u) | (b << 16u) | 0xff000000u;
}
