// TT3 块纹理的**采样**（块内解码 + 双线性 + wrap），与 trit-texture 的 CPU 参考对拍。
//
// 与 `texture.wgsl` 的区别：那条解码整图；这条只解采样点附近的 4 个纹素，
// 块流常驻显存、片元/计算着色器按需取。语义必须与 CPU `sample::sample_bilinear_with`
// 完全一致：纹素中心约定、clamp-to-edge/repeat/mirror、f32 权重、round 后夹到 0..255。
//
// 块布局（每块 12 字节 = 3 个 u32，GPU 按 LE 读）：
//   word0: e0hi e0lo e1hi e1lo（e0/e1 为 RGB565 的 BE u16）
//   word1-2: 40 纹素 × 1 trit，5 trit/字节（基 3 打包，低位在前）

struct Params {
    width: u32,
    height: u32,
    block_cols: u32,
    // 0 = clamp, 1 = repeat, 2 = mirror
    wrap: u32,
    count: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
};

@group(0) @binding(0) var<storage, read> blocks: array<u32>;
@group(0) @binding(1) var<storage, read> points: array<vec2<f32>>;
@group(0) @binding(2) var<storage, read_write> out: array<u32>;
@group(0) @binding(3) var<uniform> params: Params;

fn expand5(v: u32) -> u32 {
    return (v << 3u) | (v >> 2u);
}

fn expand6(v: u32) -> u32 {
    return (v << 2u) | (v >> 4u);
}

fn payload_byte(base: u32, k: u32) -> u32 {
    let word = blocks[base + 1u + k / 4u];
    return (word >> ((k % 4u) * 8u)) & 0xffu;
}

// 与 CPU 的 wrap_index 同一套规则（负数先抬到非负再取模）。
fn wrap_index(i: i32, n: i32, mode: u32) -> i32 {
    if (mode == 0u) {
        return clamp(i, 0, n - 1);
    }
    if (mode == 1u) {
        return ((i % n) + n) % n;
    }
    let period = 2 * n;
    let m = ((i % period) + period) % period;
    if (m < n) {
        return m;
    }
    return period - 1 - m;
}

fn texel_rgb(x_in: i32, y_in: i32) -> vec3<f32> {
    let w = i32(params.width);
    let h = i32(params.height);
    let x = wrap_index(x_in, w, params.wrap);
    let y = wrap_index(y_in, h, params.wrap);
    let block_col = u32(x / 4);
    let block_row = u32(y / 10);
    let block_id = block_row * params.block_cols + block_col;
    let index = u32(y % 10) * 4u + u32(x % 4);
    let base = block_id * 3u;
    let b0 = blocks[base] & 0xffu;
    let b1 = (blocks[base] >> 8u) & 0xffu;
    let b2 = (blocks[base] >> 16u) & 0xffu;
    let b3 = (blocks[base] >> 24u) & 0xffu;
    let e0 = (b0 << 8u) | b1;
    let e1 = (b2 << 8u) | b3;
    let r0 = expand5((e0 >> 11u) & 0x1fu);
    let g0 = expand6((e0 >> 5u) & 0x3fu);
    let b0c = expand5(e0 & 0x1fu);
    let r1 = expand5((e1 >> 11u) & 0x1fu);
    let g1 = expand6((e1 >> 5u) & 0x3fu);
    let b1c = expand5(e1 & 0x1fu);
    // 5 trit/字节是基 3 打包：第 i 个 trit = byte / 3^i % 3
    var pow3 = 1u;
    for (var i = 0u; i < index % 5u; i = i + 1u) {
        pow3 = pow3 * 3u;
    }
    let trit = (payload_byte(base, index / 5u) / pow3) % 3u;
    if (trit == 0u) {
        return vec3<f32>(f32(r0), f32(g0), f32(b0c));
    }
    if (trit == 1u) {
        return vec3<f32>(
            f32((r0 + r1) / 2u),
            f32((g0 + g1) / 2u),
            f32((b0c + b1c) / 2u),
        );
    }
    return vec3<f32>(f32(r1), f32(g1), f32(b1c));
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= params.count) {
        return;
    }
    var u = points[i].x;
    var v = points[i].y;
    if (params.wrap == 0u) {
        u = clamp(u, 0.0, 1.0);
        v = clamp(v, 0.0, 1.0);
    }
    let fx = u * f32(params.width) - 0.5;
    let fy = v * f32(params.height) - 0.5;
    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;
    let xi = i32(x0);
    let yi = i32(y0);
    let p00 = texel_rgb(xi, yi);
    let p10 = texel_rgb(xi + 1, yi);
    let p01 = texel_rgb(xi, yi + 1);
    let p11 = texel_rgb(xi + 1, yi + 1);
    // 与 CPU 完全同一套融合式：fma(a, 1-t, b*t)，两层都是单次舍入
    let top = fma(p00, vec3<f32>(1.0 - tx), p10 * tx);
    let bottom = fma(p01, vec3<f32>(1.0 - tx), p11 * tx);
    let c = fma(top, vec3<f32>(1.0 - ty), bottom * ty);
    let r = u32(clamp(round(c.x), 0.0, 255.0));
    let g = u32(clamp(round(c.y), 0.0, 255.0));
    let b = u32(clamp(round(c.z), 0.0, 255.0));
    out[i] = r | (g << 8u) | (b << 16u) | 0xff000000u;
}
