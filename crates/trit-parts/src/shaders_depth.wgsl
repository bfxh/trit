//! 深度头专用内核（自包含，不改共享管线）。
//!
//! 全部按 **NHWC 批次**写：`x[(b·h·w + y·w + x)·c + ic]`。`b=1` 时与单图逐点一致。
//! 目的：把 K 个样本拼进一次 dispatch，减少逐样本的主机录制开销。
//!
//! - `conv_b`：前向（可带 ReLU）；
//! - `conv_dw_k_b` / `conv_db_k_b`：权重/偏置梯度，按批求和（split-K 分片）；
//! - `conv_dx4m_b`：输入梯度，把 ReLU 掩码折进来（替代 mask_dy + conv_dx4）；
//! - `acc_add6`：6 段梯度并成一次 dispatch 加进扁平累加器。

// 与主 shader 同口径：每个工作组 64 个元素、每次分派 16384 个元素。
const ROW: u32 = 16384u;

struct ConvB {
    b: u32,
    h: u32,
    w: u32,
    cin: u32,
    cout: u32,
    kk: u32,
    pad: u32,
    mode: u32,
    relu: u32,
    _a: u32,
    _b: u32,
    _c: u32,
};

struct DwB {
    b: u32,
    h: u32,
    w: u32,
    cin: u32,
    cout: u32,
    kk: u32,
    pad: u32,
    relu: u32,
    chunk: u32,
    chunks: u32,
    _a: u32,
    _b: u32,
};

// ---------------------------------------------------------------- 前向

@group(0) @binding(0) var<uniform> cu: ConvB;
@group(0) @binding(1) var<storage, read> cx: array<f32>;
@group(0) @binding(2) var<storage, read> cwf: array<f32>;
@group(0) @binding(3) var<storage, read> cwt: array<i32>;
@group(0) @binding(4) var<storage, read> cws: array<f32>;
@group(0) @binding(5) var<storage, read> cbs: array<f32>;
@group(0) @binding(6) var<storage, read_write> cy: array<f32>;

@compute @workgroup_size(64)
fn conv_b(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x + gid.y * ROW;
    let hw = cu.h * cu.w;
    let total = cu.b * hw * cu.cout;
    if (idx >= total) {
        return;
    }
    let oc = idx % cu.cout;
    let p = idx / cu.cout;
    let ox = p % cu.w;
    let oy = (p / cu.w) % cu.h;
    let bidx = p / hw;
    let in_base = bidx * hw * cu.cin;
    var acc: f32 = 0.0;
    let half = i32(cu.pad);
    let sc = cws[oc];
    for (var ky: u32 = 0u; ky < cu.kk; ky = ky + 1u) {
        let iy = i32(oy) + i32(ky) - half;
        if (iy < 0 || iy >= i32(cu.h)) {
            continue;
        }
        for (var kx: u32 = 0u; kx < cu.kk; kx = kx + 1u) {
            let ix = i32(ox) + i32(kx) - half;
            if (ix < 0 || ix >= i32(cu.w)) {
                continue;
            }
            let base = in_base + (u32(iy) * cu.w + u32(ix)) * cu.cin;
            let wbase = (ky * cu.kk + kx) * cu.cin;
            for (var ic: u32 = 0u; ic < cu.cin; ic = ic + 1u) {
                let wi = (wbase + ic) * cu.cout + oc;
                var wv = cwf[wi];
                if (cu.mode == 1u) {
                    wv = f32(cwt[wi]) * sc;
                }
                acc = acc + cx[base + ic] * wv;
            }
        }
    }
    var v = acc + cbs[oc];
    if (cu.relu == 1u) {
        v = max(v, 0.0);
    }
    cy[idx] = v;
}

// ---------------------------------------------------------------- 权重梯度（split-K）

@group(0) @binding(0) var<uniform> du: DwB;
@group(0) @binding(1) var<storage, read> dx_in: array<f32>;
@group(0) @binding(2) var<storage, read> dy_in4: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> dw_part4: array<vec4<f32>>;

@compute @workgroup_size(64)
fn conv_dw_k_b(@builtin(global_invocation_id) gid: vec3<u32>) {
    let co4 = du.cout / 4u;
    let nw4 = du.kk * du.kk * du.cin * co4;
    let idx = gid.x + gid.y * ROW;
    if (idx >= nw4 * du.chunks) {
        return;
    }
    let w4 = idx % nw4;
    let c = idx / nw4;
    let oc4 = w4 % co4;
    let t = w4 / co4;
    let ic = t % du.cin;
    let ik = t / du.cin;
    let ky = ik / du.kk;
    let kx = ik % du.kk;
    let hw = du.h * du.w;
    let p0 = c * du.chunk;
    let p1 = min(p0 + du.chunk, du.b * hw);
    var acc = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    let half = i32(du.pad);
    for (var p: u32 = p0; p < p1; p = p + 1u) {
        let ox = p % du.w;
        let oy = (p / du.w) % du.h;
        let bidx = p / hw;
        let iy = i32(oy) + i32(ky) - half;
        let ix = i32(ox) + i32(kx) - half;
        if (iy < 0 || iy >= i32(du.h) || ix < 0 || ix >= i32(du.w)) {
            continue;
        }
        let xi = bidx * hw * du.cin + (u32(iy) * du.w + u32(ix)) * du.cin + ic;
        acc = acc + dx_in[xi] * dy_in4[p * co4 + oc4];
    }
    dw_part4[idx] = acc;
}

// ---------------------------------------------------------------- 偏置梯度（split-K）

@group(0) @binding(0) var<uniform> bu: DwB;
@group(0) @binding(1) var<storage, read> bdy: array<f32>;
@group(0) @binding(2) var<storage, read_write> bp_part: array<f32>;

@compute @workgroup_size(64)
fn conv_db_k_b(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x + gid.y * ROW;
    if (idx >= bu.cout * bu.chunks) {
        return;
    }
    let oc = idx % bu.cout;
    let c = idx / bu.cout;
    let p0 = c * bu.chunk;
    let p1 = min(p0 + bu.chunk, bu.b * bu.h * bu.w);
    var acc: f32 = 0.0;
    for (var p: u32 = p0; p < p1; p = p + 1u) {
        acc = acc + bdy[p * bu.cout + oc];
    }
    bp_part[idx] = acc;
}

// ---------------------------------------------------------------- 输入梯度（含 ReLU 掩码）

@group(0) @binding(0) var<uniform> xu: ConvB;
@group(0) @binding(1) var<storage, read> xdy: array<f32>;
@group(0) @binding(2) var<storage, read> xy: array<f32>;
@group(0) @binding(3) var<storage, read> xwe_t: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read_write> xdx: array<vec4<f32>>;

@compute @workgroup_size(64)
fn conv_dx4m_b(@builtin(global_invocation_id) gid: vec3<u32>) {
    let ci4 = xu.cin / 4u;
    let hw = xu.h * xu.w;
    let idx = gid.x + gid.y * ROW;
    if (idx >= xu.b * hw * ci4) {
        return;
    }
    let ic4 = idx % ci4;
    let p = idx / ci4;
    let ox = p % xu.w;
    let oy = (p / xu.w) % xu.h;
    let bidx = p / hw;
    let dy_base = bidx * hw * xu.cout;
    var acc = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    let half = i32(xu.pad);
    for (var ky: u32 = 0u; ky < xu.kk; ky = ky + 1u) {
        let iy = i32(oy) - i32(ky) + half;
        if (iy < 0 || iy >= i32(xu.h)) {
            continue;
        }
        for (var kx: u32 = 0u; kx < xu.kk; kx = kx + 1u) {
            let ix = i32(ox) - i32(kx) + half;
            if (ix < 0 || ix >= i32(xu.w)) {
                continue;
            }
            let ob = dy_base + (u32(iy) * xu.w + u32(ix)) * xu.cout;
            let wbase = (ky * xu.kk + kx) * xu.cout;
            for (var oc: u32 = 0u; oc < xu.cout; oc = oc + 1u) {
                acc = acc + xwe_t[(wbase + oc) * ci4 + ic4] * xdy[ob + oc];
            }
        }
    }
    // ReLU 掩码作用在**输入梯度**上（xy 与 xdx 同布局），逐通道 0/1 乘。
    let q = idx * 4u;
    acc = acc
        * vec4<f32>(
            select(0.0, 1.0, xy[q] > 0.0),
            select(0.0, 1.0, xy[q + 1u] > 0.0),
            select(0.0, 1.0, xy[q + 2u] > 0.0),
            select(0.0, 1.0, xy[q + 3u] > 0.0),
        );
    xdx[idx] = acc;
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
        case 0u: {
            return s0[i];
        }
        case 1u: {
            return s1[i];
        }
        case 2u: {
            return s2[i];
        }
        case 3u: {
            return s3[i];
        }
        case 4u: {
            return s4[i];
        }
        default: {
            return s5[i];
        }
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
