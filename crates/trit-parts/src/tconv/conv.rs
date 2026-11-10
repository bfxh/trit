/// 一层 3×3 卷积。
#[derive(Clone)]
pub struct Conv {
    /// 循环填充（纹理可平铺时用；零填充会把边界抹平，测出来接缝比 0.60）
    pub circular: bool,
    pub cin: usize,
    pub cout: usize,
    /// `cout·cin·9`，布局 `((co*cin)+ci)*9 + (dy*3+dx)`。
    pub w: Vec<f32>,
    pub b: Vec<f32>,
    /// 每输出通道的可学缩放（三值权重只有 ±1，缺它函数斜率调不了）。
    pub scale: Vec<f32>,
    /// true = 前向用三值权重（STE 反传）；false = 原生 f32（用于梯度校验）。
    pub ternary: bool,
    /// 膨胀率（3×3 核按 `dil` 步长取样）：感受野 = 2·dil·层数 + 1，算力不变。
    pub dil: usize,
    // Adam 状态（一阶/二阶矩）
    mw: Vec<f32>,
    vw: Vec<f32>,
    mb: Vec<f32>,
    vb: Vec<f32>,
    t: u32,
}

impl Conv {
    fn eff(&self, v: f32) -> f32 {
        if self.ternary { q(v) } else { v }
    }
}

/// 一条训练样本：(输入通道图, 目标图, 类别)。
pub type Sample = (Vec<Vec<f32>>, Vec<f32>, Option<usize>);

pub(crate) fn sobel(img: &[f32], w: usize, h: usize) -> (Vec<f32>, Vec<f32>) {
    let mut gx = vec![0.0f32; w * h];
    let mut gy = vec![0.0f32; w * h];
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let at = |dx: isize, dy: isize| {
                img[((y as isize + dy) as usize) * w + (x as isize + dx) as usize]
            };
            // gx = 水平差分（[-1 0 1; -2 0 2; -1 0 1]），gy = 竖直差分
            gx[y * w + x] =
                -at(-1, -1) - 2.0 * at(0, -1) - at(1, -1) + at(-1, 1) + 2.0 * at(0, 1) + at(1, 1);
            gy[y * w + x] =
                -at(-1, -1) - 2.0 * at(-1, 0) - at(-1, 1) + at(1, -1) + 2.0 * at(1, 0) + at(1, 1);
        }
    }
    (gx, gy)
}

/// Sobel 的**真转置**（单测 `sobel_adjoint_is_transpose` 用内积恒等式钉住）。
pub(crate) fn sobel_adjoint(cx: &[f32], cy: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; w * h];
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let (a, b) = (cx[y * w + x], cy[y * w + x]);
            out[(y - 1) * w + x - 1] += -a - b;
            out[(y - 1) * w + x] += -2.0 * a;
            out[(y - 1) * w + x + 1] += -a + b;
            out[y * w + x - 1] += -2.0 * b;
            out[y * w + x + 1] += 2.0 * b;
            out[(y + 1) * w + x - 1] += a - b;
            out[(y + 1) * w + x] += 2.0 * a;
            out[(y + 1) * w + x + 1] += a + b;
        }
    }
    out
}

/// coherence 损失与其对 x̂0 的解析梯度（抽出来做有限差分对拍）。
pub(crate) fn coh_loss_grad(
    x0: &[f32],
    w: usize,
    h: usize,
    cstar: f32,
    tstar: f32,
    lam: f32,
    w_t: f32,
) -> (f32, Vec<f32>) {
    let n = w * h;
    let (gx, gy) = sobel(x0, w, h);
    let inner = (w.saturating_sub(2)) * (h.saturating_sub(2));
    let (mut jxx, mut jyy, mut jxy) = (0.0f32, 0.0f32, 0.0f32);
    for i in 0..n {
        jxx += gx[i] * gx[i];
        jyy += gy[i] * gy[i];
        jxy += gx[i] * gy[i];
    }
    let inv = 1.0 / inner.max(1) as f32;
    let (jxx, jyy, jxy) = (jxx * inv, jyy * inv, jxy * inv);
    let tr = (jxx + jyy).max(1e-8);
    let (aa, bb) = (jxx - jyy, 2.0 * jxy);
    let dd = (aa * aa + bb * bb).sqrt().max(1e-8);
    let c = (dd / tr).clamp(0.0, 1.0);
    let et = (tr - tstar) / tstar.max(1e-6);
    let loss = lam * (c - cstar) * (c - cstar) + w_t * lam * et * et;
    let dl_dc = 2.0 * lam * (c - cstar);
    let dl_dt = 2.0 * w_t * lam * et / tstar.max(1e-6);
    let djxx = dl_dc * ((aa / dd - c) / tr) + dl_dt;
    let djyy = dl_dc * ((-aa / dd - c) / tr) + dl_dt;
    let djxy = dl_dc * ((2.0 * bb / dd) / tr);
    let cx: Vec<f32> = (0..n)
        .map(|i| (2.0 * djxx * gx[i] + djxy * gy[i]) * inv)
        .collect();
    let cy: Vec<f32> = (0..n)
        .map(|i| (2.0 * djyy * gy[i] + djxy * gx[i]) * inv)
        .collect();
    let grad = sobel_adjoint(&cx, &cy, w, h);
    (loss, grad)
}

fn q(v: f32) -> f32 {
    v.round().clamp(-1.0, 1.0)
}

impl Conv {
    pub fn new(cin: usize, cout: usize, seed: u64) -> Self {
        let mut r = seed | 1;
        let mut next = || {
            r ^= r << 13;
            r ^= r >> 7;
            r ^= r << 17;
            ((r % 2001) as f32 - 1000.0) / 2500.0
        };
        Conv {
            circular: false,
            cin,
            cout,
            // 初始要越过三值死区（|w|<0.5 → 量化成 0），否则整层恒零
            w: (0..cout * cin * 9).map(|_| next() * 3.0).collect(),
            b: vec![0.0; cout],
            ternary: true,
            scale: vec![0.1; cout],
            dil: 1,
            mw: vec![0.0; cout * cin * 9],
            vw: vec![0.0; cout * cin * 9],
            mb: vec![0.0; cout],
            vb: vec![0.0; cout],
            t: 0,
        }
    }

    fn idx(&self, co: usize, ci: usize, k: usize) -> usize {
        (co * self.cin + ci) * 9 + k
    }

    #[allow(clippy::needless_range_loop)] // 三重坐标循环，索引写法更直白
    pub fn forward(&self, x: &[Vec<f32>], w: usize, h: usize) -> Vec<Vec<f32>> {
        let mut out = vec![vec![0.0f32; w * h]; self.cout];
        let d = self.dil as i32;
        for co in 0..self.cout {
            for y in 0..h {
                for xx in 0..w {
                    let mut s = self.b[co];
                    for ci in 0..self.cin {
                        for dy in 0..3 {
                            let yyi = y as i32 + dy as i32 * d - d;
                            for dx in 0..3 {
                                let xri = xx as i32 + dx as i32 * d - d;
                                let (yy, xr) = if self.circular {
                                    (
                                        yyi.rem_euclid(h as i32) as usize,
                                        xri.rem_euclid(w as i32) as usize,
                                    )
                                } else {
                                    if yyi < 0 || yyi >= h as i32 || xri < 0 || xri >= w as i32 {
                                        continue;
                                    }
                                    (yyi as usize, xri as usize)
                                };
                                s += self.eff(self.w[self.idx(co, ci, dy * 3 + dx)])
                                    * x[ci][yy * w + xr];
                            }
                        }
                    }
                    out[co][y * w + xx] = s * self.scale[co];
                }
            }
        }
        out
    }

    /// 反向：给定 `dL/dout`，返回 `(dL/dx, dL/dw, dL/db)`；**不**更新权重（批量累积后统一更新）。
    #[allow(clippy::needless_range_loop)] // 同上
    pub(crate) fn backward(
        &self,
        x: &[Vec<f32>],
        w: usize,
        h: usize,
        dout: &[Vec<f32>],
    ) -> (Vec<Vec<f32>>, Vec<f32>, Vec<f32>, Vec<f32>) {
        // out = scale[co]·z + b[co]，z = Σ q(w)·x
        let mut dx = vec![vec![0.0f32; w * h]; self.cin];
        let mut dw = vec![0.0f32; self.w.len()];
        let mut db = vec![0.0f32; self.cout];
        let mut dscale = vec![0.0f32; self.cout];
        let d = self.dil as i32;
        for co in 0..self.cout {
            let sc = self.scale[co];
            for y in 0..h {
                for xx in 0..w {
                    let g = dout[co][y * w + xx];
                    if g == 0.0 {
                        continue;
                    }
                    db[co] += g;
                    let gz = g * sc; // 对未缩放和 z 的梯度
                    let mut z = 0.0f32;
                    for ci in 0..self.cin {
                        for dy in 0..3 {
                            let yyi = y as i32 + dy as i32 * d - d;
                            for dxk in 0..3 {
                                let xri = xx as i32 + dxk as i32 * d - d;
                                let (yy, xr) = if self.circular {
                                    (
                                        yyi.rem_euclid(h as i32) as usize,
                                        xri.rem_euclid(w as i32) as usize,
                                    )
                                } else {
                                    if yyi < 0 || yyi >= h as i32 || xri < 0 || xri >= w as i32 {
                                        continue;
                                    }
                                    (yyi as usize, xri as usize)
                                };
                                let k = dy * 3 + dxk;
                                let xi = yy * w + xr;
                                let xv = x[ci][xi];
                                let qw = self.eff(self.w[self.idx(co, ci, k)]);
                                z += qw * xv;
                                dw[self.idx(co, ci, k)] += gz * xv;
                                dx[ci][xi] += gz * qw;
                            }
                        }
                    }
                    dscale[co] += g * z;
                }
            }
        }
        (dx, dw, db, dscale)
    }

    /// 用累积梯度更新（`adam=false` → SGD；否则 Adam，梯度已含 1/N）。
    #[allow(clippy::needless_range_loop)] // 三路并行索引（w/m/v）
    pub(crate) fn apply(&mut self, dw: &[f32], db: &[f32], dsc: &[f32], lr: f32, adam: bool) {
        if adam {
            self.t += 1;
            let (b1, b2, eps) = (0.9f32, 0.999f32, 1e-8f32);
            let c1 = 1.0 - b1.powi(self.t as i32);
            let c2 = 1.0 - b2.powi(self.t as i32);
            for i in 0..self.w.len() {
                self.mw[i] = b1 * self.mw[i] + (1.0 - b1) * dw[i];
                self.vw[i] = b2 * self.vw[i] + (1.0 - b2) * dw[i] * dw[i];
                self.w[i] -= lr * (self.mw[i] / c1) / ((self.vw[i] / c2).sqrt() + eps);
            }
            for i in 0..self.b.len() {
                self.mb[i] = b1 * self.mb[i] + (1.0 - b1) * db[i];
                self.vb[i] = b2 * self.vb[i] + (1.0 - b2) * db[i] * db[i];
                self.b[i] -= lr * (self.mb[i] / c1) / ((self.vb[i] / c2).sqrt() + eps);
                // scale 用普通 SGD（Adam 状态只留给权重/bias）
                self.scale[i] = (self.scale[i] - lr * dsc[i]).clamp(1e-3, 10.0);
            }
        } else {
            for (wi, gi) in self.w.iter_mut().zip(dw) {
                *wi -= lr * gi;
            }
            for (bi, gi) in self.b.iter_mut().zip(db) {
                *bi -= lr * gi;
            }
            for (si, gi) in self.scale.iter_mut().zip(dsc) {
                *si = (*si - lr * gi).clamp(1e-3, 10.0);
            }
        }
    }
}
