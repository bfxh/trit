// 部件平面头的反向内核（与 shaders.wgsl 的布局严格一致）。
//
// 只算「对有效权重 E」的梯度（E = q·scale，mode=0 时 E = wf）：
//   conv_dx / conv_dw / conv_db / pool_dx / cat_dx / convt_dx / convt_dw / convt_db
// E → latent 的 STE 映射在主机侧做（见 backward.rs 的 ste 段）。
// 所有输出元素按下标 idx = gid.x + gid.y * ROW 展开，ROW = 256 组 × 64 线程。

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

// ---------------------------------------------------------------- 卷积：输入梯度

@group(0) @binding(0) var<uniform> u: ConvU;
@group(0) @binding(1) var<storage, read> y: array<f32>; // 前向输出（ReLU 掩码用）
@group(0) @binding(2) var<storage, read> dy: array<f32>;
@group(0) @binding(3) var<storage, read> wf: array<f32>;
@group(0) @binding(4) var<storage, read> wt: array<i32>;
@group(0) @binding(5) var<storage, read> ws: array<f32>;
@group(0) @binding(6) var<storage, read_write> dx: array<f32>;

@compute @workgroup_size(64)
fn conv_dx(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x + gid.y * ROW;
    if (idx >= u.h * u.w * u.cin) {
        return;
    }
    let ic = idx % u.cin;
    let p = idx / u.cin;
    let ix = p % u.w;
    let iy = p / u.w;
    var acc: f32 = 0.0;
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
            let wbase = (ky * u.kk + kx) * u.cin;
            for (var oc: u32 = 0u; oc < u.cout; oc = oc + 1u) {
                let yi = ob + oc;
                var g = dy[yi];
                if (u.relu == 1u && y[yi] <= 0.0) {
                    g = 0.0;
                }
                let wi = (wbase + ic) * u.cout + oc;
                var wv = wf[wi];
                if (u.mode == 1u) {
                    wv = f32(wt[wi]) * ws[oc];
                }
                acc = acc + wv * g;
            }
        }
    }
    dx[idx] = acc;
}

// ---------------------------------------------------------------- 卷积：权重梯度

// 绑定顺序不同，用独立变量名。
@group(0) @binding(0) var<uniform> wu: ConvU;
@group(0) @binding(1) var<storage, read> wx: array<f32>;
@group(0) @binding(2) var<storage, read> wy: array<f32>;
@group(0) @binding(3) var<storage, read> wdy: array<f32>;
@group(0) @binding(4) var<storage, read_write> dw: array<f32>;

@compute @workgroup_size(64)
fn conv_dw(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x + gid.y * ROW;
    let nw = wu.kk * wu.kk * wu.cin * wu.cout;
    if (idx >= nw) {
        return;
    }
    let oc = idx % wu.cout;
    let t = idx / wu.cout;
    let ic = t % wu.cin;
    let ik = t / wu.cin;
    let ky = ik / wu.kk;
    let kx = ik % wu.kk;
    var acc: f32 = 0.0;
    let half = i32(wu.pad);
    for (var oy: u32 = 0u; oy < wu.h; oy = oy + 1u) {
        let iy = i32(oy) + i32(ky) - half;
        if (iy < 0 || iy >= i32(wu.h)) {
            continue;
        }
        for (var ox: u32 = 0u; ox < wu.w; ox = ox + 1u) {
            let ix = i32(ox) + i32(kx) - half;
            if (ix < 0 || ix >= i32(wu.w)) {
                continue;
            }
            let oi = (oy * wu.w + ox) * wu.cout + oc;
            var g = wdy[oi];
            if (wu.relu == 1u && wy[oi] <= 0.0) {
                g = 0.0;
            }
            acc = acc + wx[((u32(iy) * wu.w + u32(ix)) * wu.cin) + ic] * g;
        }
    }
    dw[idx] = acc;
}

// ---------------------------------------------------------------- 卷积：偏置梯度

@group(0) @binding(0) var<uniform> bu: ConvU;
@group(0) @binding(1) var<storage, read> by: array<f32>;
@group(0) @binding(2) var<storage, read> bdy: array<f32>;
@group(0) @binding(3) var<storage, read_write> dbs: array<f32>;

@compute @workgroup_size(64)
fn conv_db(@builtin(global_invocation_id) gid: vec3<u32>) {
    let oc = gid.x + gid.y * ROW;
    if (oc >= bu.cout) {
        return;
    }
    var acc: f32 = 0.0;
    for (var p: u32 = 0u; p < bu.h * bu.w; p = p + 1u) {
        let oi = p * bu.cout + oc;
        var g = bdy[oi];
        if (bu.relu == 1u && by[oi] <= 0.0) {
            g = 0.0;
        }
        acc = acc + g;
    }
    dbs[oc] = acc;
}

// ---------------------------------------------------------------- 池化反向

struct PoolU { h: u32, w: u32, c: u32, _p: u32 };
@group(0) @binding(0) var<uniform> pu: PoolU;
@group(0) @binding(1) var<storage, read> px: array<f32>;
@group(0) @binding(2) var<storage, read> pdy: array<f32>;
@group(0) @binding(3) var<storage, read_write> pdx: array<f32>;

@compute @workgroup_size(64)
fn pool_dx(@builtin(global_invocation_id) gid: vec3<u32>) {
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
    var best = -3.402823e38;
    var by: u32 = 0u;
    var bx: u32 = 0u;
    for (var dyi: u32 = 0u; dyi < 2u; dyi = dyi + 1u) {
        for (var dxi: u32 = 0u; dxi < 2u; dxi = dxi + 1u) {
            let v = px[((oy * 2u + dyi) * pu.w + ox * 2u + dxi) * pu.c + c];
            if (v > best) {
                best = v;
                by = dyi;
                bx = dxi;
            }
        }
    }
    let at = ((oy * 2u + by) * pu.w + ox * 2u + bx) * pu.c + c;
    pdx[at] = pdy[idx];
}

// ---------------------------------------------------------------- 拼接反向

struct CatU { hw: u32, ca: u32, cb: u32, _p: u32 };
@group(0) @binding(0) var<uniform> cu: CatU;
@group(0) @binding(1) var<storage, read> cdy: array<f32>;
@group(0) @binding(2) var<storage, read_write> cda: array<f32>;
@group(0) @binding(3) var<storage, read_write> cdb: array<f32>;

@compute @workgroup_size(64)
fn cat_dx(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x + gid.y * ROW;
    let co = cu.ca + cu.cb;
    if (idx >= cu.hw * co) {
        return;
    }
    let c = idx % co;
    let p = idx / co;
    if (c < cu.ca) {
        cda[p * cu.ca + c] = cdy[idx];
    } else {
        cdb[p * cu.cb + (c - cu.ca)] = cdy[idx];
    }
}

// ---------------------------------------------------------------- 转置卷积反向

struct UpU { h: u32, w: u32, cin: u32, cout: u32, mode: u32, _a: u32, _b: u32, _c: u32 };
@group(0) @binding(0) var<uniform> uu: UpU;
@group(0) @binding(1) var<storage, read> udy: array<f32>;
@group(0) @binding(2) var<storage, read> uwf: array<f32>;
@group(0) @binding(3) var<storage, read> uwt: array<i32>;
@group(0) @binding(4) var<storage, read> uws: array<f32>;
@group(0) @binding(5) var<storage, read_write> udx: array<f32>;

@compute @workgroup_size(64)
fn convt_dx(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x + gid.y * ROW;
    if (idx >= uu.h * uu.w * uu.cin) {
        return;
    }
    let ci = idx % uu.cin;
    let p = idx / uu.cin;
    let ix = p % uu.w;
    let iy = p / uu.w;
    let ow = uu.w * 2u;
    var acc: f32 = 0.0;
    for (var kh: u32 = 0u; kh < 2u; kh = kh + 1u) {
        for (var kw: u32 = 0u; kw < 2u; kw = kw + 1u) {
            let ob = ((iy * 2u + kh) * ow + ix * 2u + kw) * uu.cout;
            let wbase = (kh * 2u + kw) * uu.cin;
            for (var co: u32 = 0u; co < uu.cout; co = co + 1u) {
                let wi = (wbase + ci) * uu.cout + co;
                var wv = uwf[wi];
                if (uu.mode == 1u) {
                    wv = f32(uwt[wi]) * uws[co];
                }
                acc = acc + wv * udy[ob + co];
            }
        }
    }
    udx[idx] = acc;
}

@group(0) @binding(0) var<uniform> gu: UpU;
@group(0) @binding(1) var<storage, read> gx: array<f32>;
@group(0) @binding(2) var<storage, read> gdy: array<f32>;
@group(0) @binding(3) var<storage, read_write> gdw: array<f32>;

@compute @workgroup_size(64)
fn convt_dw(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x + gid.y * ROW;
    let nw = 4u * gu.cin * gu.cout;
    if (idx >= nw) {
        return;
    }
    let co = idx % gu.cout;
    let t = idx / gu.cout;
    let ci = t % gu.cin;
    let sub = t / gu.cin;
    let kh = sub / 2u;
    let kw = sub % 2u;
    let ow = gu.w * 2u;
    var acc: f32 = 0.0;
    for (var iy: u32 = 0u; iy < gu.h; iy = iy + 1u) {
        for (var ix: u32 = 0u; ix < gu.w; ix = ix + 1u) {
            let g = gdy[((iy * 2u + kh) * ow + ix * 2u + kw) * gu.cout + co];
            acc = acc + gx[(iy * gu.w + ix) * gu.cin + ci] * g;
        }
    }
    gdw[idx] = acc;
}

@group(0) @binding(0) var<uniform> hu: UpU;
@group(0) @binding(1) var<storage, read> hdy: array<f32>;
@group(0) @binding(2) var<storage, read_write> hdb: array<f32>;

@compute @workgroup_size(64)
fn convt_db(@builtin(global_invocation_id) gid: vec3<u32>) {
    let co = gid.x + gid.y * ROW;
    if (co >= hu.cout) {
        return;
    }
    var acc: f32 = 0.0;
    for (var p: u32 = 0u; p < (hu.h * 2u) * (hu.w * 2u); p = p + 1u) {
        acc = acc + hdy[p * hu.cout + co];
    }
    hdb[co] = acc;
}
