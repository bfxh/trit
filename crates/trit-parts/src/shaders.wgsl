// 部件平面头的三进制前向内核。
//
// 统一布局：激活为 NHWC（N=1），f32；权重两种形态
//   mode=0  f32 稠密（BN 已折叠进权重与 bias）——用于与 Python ckpt 逐元素对拍
//   mode=1  三值 i8 {-1,0,+1} + 每输出通道 scale —— 整数累加，乘加只剩加减
// 所有输出元素按下标 idx = gid.x + gid.y * ROW 展开，ROW = 256 个工作组 × 64 线程。

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
@group(0) @binding(1) var<storage, read> x: array<f32>;
@group(0) @binding(2) var<storage, read> wf: array<f32>;
@group(0) @binding(3) var<storage, read> wt: array<i32>;
@group(0) @binding(4) var<storage, read> ws: array<f32>;
@group(0) @binding(5) var<storage, read> bs: array<f32>;
@group(0) @binding(6) var<storage, read_write> y: array<f32>;

@compute @workgroup_size(64)
fn conv(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x + gid.y * ROW;
    let total = u.h * u.w * u.cout;
    if (idx >= total) {
        return;
    }
    let oc = idx % u.cout;
    let p = idx / u.cout;
    let ox = p % u.w;
    let oy = p / u.w;

    var acc: f32 = 0.0;
    let half = i32(u.pad);
    let sc = ws[oc];
    for (var ky: u32 = 0u; ky < u.kk; ky = ky + 1u) {
        let iy = i32(oy) + i32(ky) - half;
        if (iy < 0 || iy >= i32(u.h)) {
            continue;
        }
        for (var kx: u32 = 0u; kx < u.kk; kx = kx + 1u) {
            let ix = i32(ox) + i32(kx) - half;
            if (ix < 0 || ix >= i32(u.w)) {
                continue;
            }
            let base = (u32(iy) * u.w + u32(ix)) * u.cin;
            let wbase = (ky * u.kk + kx) * u.cin;
            for (var ic: u32 = 0u; ic < u.cin; ic = ic + 1u) {
                let wi = (wbase + ic) * u.cout + oc;
                // 三值：整数权重 × 每通道 scale；f32：直接用
                var wv = wf[wi];
                if (u.mode == 1u) {
                    wv = f32(wt[wi]) * sc;
                }
                acc = acc + x[base + ic] * wv;
            }
        }
    }
    var v = acc + bs[oc];
    if (u.relu == 1u) {
        v = max(v, 0.0);
    }
    y[idx] = v;
}

// 2x2 stride-2 最大池化
struct PoolU { h: u32, w: u32, c: u32, _p: u32 };
@group(0) @binding(0) var<uniform> pu: PoolU;
@group(0) @binding(1) var<storage, read> px: array<f32>;
@group(0) @binding(2) var<storage, read_write> py: array<f32>;

@compute @workgroup_size(64)
fn pool(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x + gid.y * ROW;
    let oh = pu.h / 2u;
    let ow = pu.w / 2u;
    if (idx >= oh * ow * pu.c) {
        return;
    }
    let c = idx % pu.c;
    let p = idx / pu.c;
    let ox = p % ow;
    let oy = p / ow;
    var m = -3.402823e38;
    for (var dy: u32 = 0u; dy < 2u; dy = dy + 1u) {
        for (var dx: u32 = 0u; dx < 2u; dx = dx + 1u) {
            let v = px[((oy * 2u + dy) * pu.w + (ox * 2u + dx)) * pu.c + c];
            m = max(m, v);
        }
    }
    py[idx] = m;
}

// 通道拼接：y = [a | b]（NHWC，沿通道轴）
struct CatU { hw: u32, ca: u32, cb: u32, _p: u32 };
@group(0) @binding(0) var<uniform> cu: CatU;
@group(0) @binding(1) var<storage, read> ca_buf: array<f32>;
@group(0) @binding(2) var<storage, read> cb_buf: array<f32>;
@group(0) @binding(3) var<storage, read_write> cy: array<f32>;

@compute @workgroup_size(64)
fn cat(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x + gid.y * ROW;
    let co = cu.ca + cu.cb;
    if (idx >= cu.hw * co) {
        return;
    }
    let c = idx % co;
    let p = idx / co;
    if (c < cu.ca) {
        cy[idx] = ca_buf[p * cu.ca + c];
    } else {
        cy[idx] = cb_buf[p * cu.cb + (c - cu.ca)];
    }
}

// ConvTranspose2d(k=2, stride=2)：等价于 4 个 1x1 卷积按子像素位置摆放。
// 权重布局 [(kh*2+kw)*cin + ci][co]
struct UpU { h: u32, w: u32, cin: u32, cout: u32, mode: u32, _a: u32, _b: u32, _c: u32 };
@group(0) @binding(0) var<uniform> uu: UpU;
@group(0) @binding(1) var<storage, read> ux: array<f32>;
@group(0) @binding(2) var<storage, read> uwf: array<f32>;
@group(0) @binding(3) var<storage, read> uwt: array<i32>;
@group(0) @binding(4) var<storage, read> uws: array<f32>;
@group(0) @binding(5) var<storage, read> ubs: array<f32>;
@group(0) @binding(6) var<storage, read_write> uy: array<f32>;

@compute @workgroup_size(64)
fn convt(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x + gid.y * ROW;
    let oh = uu.h * 2u;
    let ow = uu.w * 2u;
    if (idx >= oh * ow * uu.cout) {
        return;
    }
    let co = idx % uu.cout;
    let p = idx / uu.cout;
    let ox = p % ow;
    let oy = p / ow;
    let iy = oy / 2u;
    let ix = ox / 2u;
    let sub = (oy % 2u) * 2u + (ox % 2u);
    let base = (iy * uu.w + ix) * uu.cin;
    let sc = uws[co];
    var acc: f32 = 0.0;
    for (var ci: u32 = 0u; ci < uu.cin; ci = ci + 1u) {
        let wi = (sub * uu.cin + ci) * uu.cout + co;
        var wv = uwf[wi];
        if (uu.mode == 1u) {
            wv = f32(uwt[wi]) * sc;
        }
        acc = acc + ux[base + ci] * wv;
    }
    // ⚠️ 转置卷积也有 bias（ConvTranspose2d 默认 bias=True）；漏掉会造成整体偏移
    uy[idx] = acc + ubs[co];
}
