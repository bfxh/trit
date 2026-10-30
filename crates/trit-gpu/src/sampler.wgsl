// TT3 块纹理的**三线性采样**（mip 链 + 双线性 + wrap），与 trit-texture 的 CPU 参考对拍。
//
// 块流按级拼接上传一次；每级带 (块偏移, 宽, 高, 块列数)。采样点带 `lod`：
// 在相邻两级各做一次双线性，再按 `lod` 的小数部分融合。
//
// 语义必须与 CPU `TtxDocument::sample_trilinear_with` 完全一致：
// 纹素中心、clamp/repeat/mirror、显式 fma（单次舍入）、round ties-to-even。
//
// 块布局（每块 12 字节 = 3 个 u32，GPU 按 LE 读）：
//   word0: e0hi e0lo e1hi e1lo（e0/e1 为 RGB565 的 BE u16）
//   word1-2: 40 纹素 × 1 trit，5 trit/字节（基 3 打包，低位在前）

struct Level {
    // 该级在块流里的起始块序号
    block_offset: u32,
    width: u32,
    height: u32,
    block_cols: u32,
    // 图集子矩形（该级纹素坐标）；rect_w/rect_h = 0 表示整级
    rect_x: u32,
    rect_y: u32,
    rect_w: u32,
    rect_h: u32,
};

struct Params {
    level_count: u32,
    // 0 = clamp, 1 = repeat, 2 = mirror
    wrap: u32,
    count: u32,
    pad0: u32,
};

@group(0) @binding(0) var<storage, read> blocks: array<u32>;
@group(0) @binding(1) var<storage, read> levels: array<Level>;
// 每样点：(u, v, lod, taps) + (step_u, step_v, _, _)。
// taps = 1 时 step 无意义（退化为单次三线性）；taps > 1 即各向异性多点平均。
struct Sample {
    uv_lod_taps: vec4<f32>,
    step: vec4<f32>,
};

@group(0) @binding(2) var<storage, read> points: array<Sample>;
@group(0) @binding(3) var<storage, read_write> out: array<u32>;
@group(0) @binding(4) var<uniform> params: Params;

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

fn texel_rgb(level: u32, x_in: i32, y_in: i32) -> vec3<f32> {
    let lvl = levels[level];
    // 有子矩形时 wrap 只在矩形内循环，避免采到邻居图集条目
    let rw = select(lvl.width, lvl.rect_w, lvl.rect_w > 0u);
    let rh = select(lvl.height, lvl.rect_h, lvl.rect_h > 0u);
    let rx = select(0u, lvl.rect_x, lvl.rect_w > 0u);
    let ry = select(0u, lvl.rect_y, lvl.rect_h > 0u);
    let x = i32(rx) + wrap_index(x_in, i32(rw), params.wrap);
    let y = i32(ry) + wrap_index(y_in, i32(rh), params.wrap);
    let block_col = u32(x / 4);
    let block_row = u32(y / 10);
    let block_id = block_row * lvl.block_cols + block_col;
    let index = u32(y % 10) * 4u + u32(x % 4);
    let base = (lvl.block_offset + block_id) * 3u;
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

// 回 8 位（ties-to-even），与 CPU 的逐级量化对齐
fn quantize(c: vec3<f32>) -> vec3<f32> {
    return clamp(
        vec3<f32>(round(c.x), round(c.y), round(c.z)),
        vec3<f32>(0.0),
        vec3<f32>(255.0),
    );
}

fn bilinear(level: u32, u_in: f32, v_in: f32) -> vec3<f32> {
    let lvl = levels[level];
    var u = u_in;
    var v = v_in;
    if (params.wrap == 0u) {
        u = clamp(u, 0.0, 1.0);
        v = clamp(v, 0.0, 1.0);
    }
    let rw = select(lvl.width, lvl.rect_w, lvl.rect_w > 0u);
    let rh = select(lvl.height, lvl.rect_h, lvl.rect_h > 0u);
    let fx = u * f32(rw) - 0.5;
    let fy = v * f32(rh) - 0.5;
    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;
    let xi = i32(x0);
    let yi = i32(y0);
    let p00 = texel_rgb(level, xi, yi);
    let p10 = texel_rgb(level, xi + 1, yi);
    let p01 = texel_rgb(level, xi, yi + 1);
    let p11 = texel_rgb(level, xi + 1, yi + 1);
    // 与 CPU 完全同一套融合式：fma(a, 1-t, b*t)，每层单次舍入
    let top = fma(p00, vec3<f32>(1.0 - tx), p10 * tx);
    let bottom = fma(p01, vec3<f32>(1.0 - tx), p11 * tx);
    return fma(top, vec3<f32>(1.0 - ty), bottom * ty);
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= params.count) {
        return;
    }
    let sample = points[i];
    let last = f32(params.level_count - 1u);
    var lod = sample.uv_lod_taps.z;
    // NaN 归零（与 CPU 同口径）
    if (lod != lod) {
        lod = 0.0;
    }
    lod = clamp(lod, 0.0, last);
    let base_level = floor(lod);
    let l0 = u32(base_level);
    let l1 = min(l0 + 1u, params.level_count - 1u);
    let t = lod - base_level;
    // 每级先回 8 位再融合——与 CPU 参考的量化时机一致（先量化、后混、再量化）
    // 各向异性：沿长轴在半个像素跨度内均匀取 taps 个三线性点，**每点先回 8 位再平均**
    //（与 CPU `sample_aniso` 的量化时机一致）
    let taps = max(1u, u32(sample.uv_lod_taps.w));
    var acc = vec3<f32>(0.0);
    for (var k = 0u; k < taps; k = k + 1u) {
        let off = (f32(k) + 0.5) / f32(taps) - 0.5;
        let su = sample.uv_lod_taps.x + sample.step.x * off;
        let sv = sample.uv_lod_taps.y + sample.step.y * off;
        let a = quantize(bilinear(l0, su, sv));
        var tap_color = a;
        if (l1 != l0) {
            let b = quantize(bilinear(l1, su, sv));
            tap_color = fma(a, vec3<f32>(1.0 - t), b * t);
        }
        acc = acc + clamp(
            vec3<f32>(round(tap_color.x), round(tap_color.y), round(tap_color.z)),
            vec3<f32>(0.0),
            vec3<f32>(255.0),
        );
    }
    let c = acc / f32(taps);
    let r = u32(clamp(round(c.x), 0.0, 255.0));
    let g = u32(clamp(round(c.y), 0.0, 255.0));
    let b = u32(clamp(round(c.z), 0.0, 255.0));
    out[i] = r | (g << 8u) | (b << 16u) | 0xff000000u;
}
