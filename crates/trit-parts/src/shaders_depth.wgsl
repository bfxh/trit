//! 深度头专用的融合内核（不改共享管线，避免影响 parts 训练路径）。
//!
//! 目的只是**减少 dispatch 数**，数值与原路径逐点一致：
//! - `conv_dx4m`：把 ReLU 掩码折进卷积的输入梯度（替代 mask_dy + conv_dx4 两次 dispatch）；
//! - `acc_add6`：把 6 段梯度并进一个扁平累加器（替代 6 次 add_into）。

// 与主 shader 同口径：每个工作组 64 个元素、每次分派 16384 个元素。
const ROW: u32 = 16384u;

struct ConvU {
    h: u32,
    w: u32,
    cin: u32,
    cout: u32,
    kk: u32,
    pad: u32,
    mode: u32,
    relu: u32,
};

@group(0) @binding(0) var<uniform> u: ConvU;
@group(0) @binding(1) var<storage, read> dy: array<f32>;
// 前向 ReLU 的输出（与 dx 同一布局）：`y <= 0` 的通道不传梯度。
@group(0) @binding(2) var<storage, read> y: array<f32>;
// 转置后的有效权重 [k·cout + oc]·cin + ic。
@group(0) @binding(3) var<storage, read> we_t: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read_write> dx: array<vec4<f32>>;

@compute @workgroup_size(64)
fn conv_dx4m(@builtin(global_invocation_id) gid: vec3<u32>) {
    let ci4 = u.cin / 4u;
    let idx = gid.x + gid.y * ROW;
    if (idx >= u.h * u.w * ci4) {
        return;
    }
    let ic4 = idx % ci4;
    let p = idx / ci4;
    let ix = p % u.w;
    let iy = p / u.w;
    var acc = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    let half = i32(u.pad);
    for (var ky: u32 = 0u; ky < u.kk; ky = ky + 1u) {
        let oy = i32(iy) - i32(ky) + half;
        if (oy < 0 || oy >= i32(u.h)) {
            continue;
        }
        for (var kx: u32 = 0u; kx < u.kk; kx = kx + 1u) {
            let ox = i32(ix) - i32(kx) + half;
            if (ox < 0 || ox >= i32(u.w)) {
                continue;
            }
            let ob = (u32(oy) * u.w + u32(ox)) * u.cout;
            let wbase = (ky * u.kk + kx) * u.cout;
            for (var oc: u32 = 0u; oc < u.cout; oc = oc + 1u) {
                acc = acc + we_t[(wbase + oc) * ci4 + ic4] * dy[ob + oc];
            }
        }
    }
    // ReLU 掩码作用在**输入梯度**上（y 与 dx 同布局），这里是逐通道的 0/1 乘。
    let b = idx * 4u;
    acc = acc
        * vec4<f32>(
            select(0.0, 1.0, y[b] > 0.0),
            select(0.0, 1.0, y[b + 1u] > 0.0),
            select(0.0, 1.0, y[b + 2u] > 0.0),
            select(0.0, 1.0, y[b + 3u] > 0.0),
        );
    dx[idx] = acc;
}

// ---------------------------------------------------------------- 梯度累加（6 段 → 1 段）

struct AccU {
    total: u32,
    nseg: u32,
    _a: u32,
    _b: u32,
    // x = 目标起点（扁平缓冲下标），y = 长度
    seg: array<vec4<u32>, 8>,
};

@group(0) @binding(0) var<uniform> au: AccU;
@group(0) @binding(1) var<storage, read> s0: array<f32>;
@group(0) @binding(2) var<storage, read> s1: array<f32>;
@group(0) @binding(3) var<storage, read> s2: array<f32>;
@group(0) @binding(4) var<storage, read> s3: array<f32>;
@group(0) @binding(5) var<storage, read> s4: array<f32>;
@group(0) @binding(6) var<storage, read> s5: array<f32>;
@group(0) @binding(7) var<storage, read_write> dst: array<f32>;

fn seg_val(s: u32, i: u32) -> f32 {
    switch s {
        case 0u: { return s0[i]; }
        case 1u: { return s1[i]; }
        case 2u: { return s2[i]; }
        case 3u: { return s3[i]; }
        case 4u: { return s4[i]; }
        default: { return s5[i]; }
    }
}

@compute @workgroup_size(64)
fn acc_add6(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x + gid.y * ROW;
    if (i >= au.total) {
        return;
    }
    for (var s: u32 = 0u; s < au.nseg; s = s + 1u) {
        let off = au.seg[s].x;
        let len = au.seg[s].y;
        if (i >= off && i < off + len) {
            dst[i] = dst[i] + seg_val(s, i - off);
            return;
        }
    }
}
