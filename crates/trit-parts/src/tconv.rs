//! 小型三值卷积栈（CPU）：3×3、same padding、隐藏层 ReLU、输出 sigmoid + BCE。
//!
//! 用途：证明「扩感受野要靠加深 + 共享权重」而不是把 patch 摊平进 MLP。
//! 权重三值（{-1,0,+1}，STE 直通），bias 与输出层保持 f32（与 `occ::Mlp` 同一取舍）。
//! 通道输入：第 0 通道是图像，后面可拼 one-hot 条件（常数图）。

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
/// FiLM 反向所需：(FiLM 前的 conv 输出, 该类 γ, 类别)。
type FilmState = (Vec<Vec<f32>>, Vec<f32>, Option<usize>);

fn sobel(img: &[f32], w: usize, h: usize) -> (Vec<f32>, Vec<f32>) {
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
fn sobel_adjoint(cx: &[f32], cy: &[f32], w: usize, h: usize) -> Vec<f32> {
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

/// 批归一化（单图训练 ⇒ 按通道在**空间维**归一化），带 running stats 供评测。
#[derive(Clone)]
pub struct Bn {
    pub gamma: Vec<f32>,
    pub beta: Vec<f32>,
    pub mean: Vec<f32>,
    pub var: Vec<f32>,
    pub momentum: f32,
    /// 前向实际用的批统计（反向必须用同一份）。
    batch_mean: Vec<f32>,
    batch_var: Vec<f32>,
    // Adam 状态
    mg: Vec<f32>,
    vg: Vec<f32>,
    mb: Vec<f32>,
    vb: Vec<f32>,
    t: u32,
}

impl Bn {
    pub fn new(c: usize) -> Self {
        Bn {
            gamma: vec![1.0; c],
            beta: vec![0.0; c],
            mean: vec![0.0; c],
            var: vec![1.0; c],
            momentum: 0.9,
            batch_mean: vec![0.0; c],
            batch_var: vec![1.0; c],
            mg: vec![0.0; c],
            vg: vec![0.0; c],
            mb: vec![0.0; c],
            vb: vec![0.0; c],
            t: 0,
        }
    }

    /// 前向：返回 `(γ·x̂+β, x̂)`；`train` 时更新 running stats。
    pub fn forward(
        &mut self,
        x: &[Vec<f32>],
        n: usize,
        train: bool,
    ) -> (Vec<Vec<f32>>, Vec<Vec<f32>>) {
        let mut xh = vec![vec![0.0f32; n]; x.len()];
        let mut y = vec![vec![0.0f32; n]; x.len()];
        for c in 0..x.len() {
            let mu = x[c].iter().sum::<f32>() / n as f32;
            let var = x[c].iter().map(|v| (v - mu) * (v - mu)).sum::<f32>() / n as f32;
            if train {
                self.mean[c] = self.momentum * self.mean[c] + (1.0 - self.momentum) * mu;
                self.var[c] = self.momentum * self.var[c] + (1.0 - self.momentum) * var;
            }
            self.batch_mean[c] = mu;
            self.batch_var[c] = var;
            let (m, v) = if train {
                (mu, var)
            } else {
                (self.mean[c], self.var[c])
            };
            let inv = 1.0 / (v + 1e-5).sqrt();
            for i in 0..n {
                let z = (x[c][i] - m) * inv;
                xh[c][i] = z;
                y[c][i] = self.gamma[c] * z + self.beta[c];
            }
        }
        (y, xh)
    }

    /// 反向：`xhat` 是归一化后的值（不含 γ/β）。返回 `(dx, dγ, dβ)`，不更新。
    pub fn backward(
        &self,
        xhat: &[Vec<f32>],
        dout: &[Vec<f32>],
        n: usize,
    ) -> (Vec<Vec<f32>>, Vec<f32>, Vec<f32>) {
        let mut dx = vec![vec![0.0f32; n]; dout.len()];
        let mut dgs = Vec::with_capacity(dout.len());
        let mut dbs = Vec::with_capacity(dout.len());
        for c in 0..dout.len() {
            let dgamma: f32 = dout[c].iter().zip(&xhat[c]).map(|(d, z)| d * z).sum();
            let dbeta: f32 = dout[c].iter().sum();
            let mean_d = dbeta / n as f32;
            let mean_dx: f32 = dout[c]
                .iter()
                .zip(&xhat[c])
                .map(|(d, z)| d * z)
                .sum::<f32>()
                / n as f32;
            // 用前向的**批**方差（不是 running var）
            let inv = 1.0 / (self.batch_var[c] + 1e-5).sqrt();
            for i in 0..n {
                dx[c][i] = self.gamma[c] * inv * (dout[c][i] - mean_d - xhat[c][i] * mean_dx);
            }
            dgs.push(dgamma);
            dbs.push(dbeta);
        }
        (dx, dgs, dbs)
    }

    /// 用累积梯度更新 γ/β（`adam` 同 Conv）。
    pub fn apply(&mut self, dg: &[f32], db: &[f32], lr: f32, adam: bool) {
        let c = self.gamma.len();
        if adam {
            self.t += 1;
            let (b1, b2, eps) = (0.9f32, 0.999f32, 1e-8f32);
            let c1 = 1.0 - b1.powi(self.t as i32);
            let c2 = 1.0 - b2.powi(self.t as i32);
            for i in 0..c {
                self.mg[i] = b1 * self.mg[i] + (1.0 - b1) * dg[i];
                self.vg[i] = b2 * self.vg[i] + (1.0 - b2) * dg[i] * dg[i];
                self.gamma[i] -= lr * (self.mg[i] / c1) / ((self.vg[i] / c2).sqrt() + eps);
                self.mb[i] = b1 * self.mb[i] + (1.0 - b1) * db[i];
                self.vb[i] = b2 * self.vb[i] + (1.0 - b2) * db[i] * db[i];
                self.beta[i] -= lr * (self.mb[i] / c1) / ((self.vb[i] / c2).sqrt() + eps);
            }
        } else {
            for i in 0..c {
                self.gamma[i] -= lr * dg[i];
                self.beta[i] -= lr * db[i];
            }
        }
    }
}

/// 辅助分类头：对隐特征做全局平均池化 → 线性 → 类别 logits。
/// 目的：给「逐点回归」补一条**对类别有可辨识性**的损失（FiLM 单独用会被忽略）。
pub struct AuxCls {
    pub w: Vec<f32>,
    pub b: Vec<f32>,
    pub nk: usize,
    pub c: usize,
}

impl AuxCls {
    pub fn new(nk: usize, c: usize) -> Self {
        AuxCls {
            w: vec![0.0; nk * c],
            b: vec![0.0; nk],
            nk,
            c,
        }
    }
    fn logits(&self, pool: &[f32]) -> Vec<f32> {
        (0..self.nk)
            .map(|k| {
                self.b[k]
                    + (0..self.c)
                        .map(|i| self.w[k * self.c + i] * pool[i])
                        .sum::<f32>()
            })
            .collect()
    }
}

/// FiLM：按类别对隐层做逐通道缩放/平移（`y = g[c]·y + b[c]`）。
/// 行数 = 类别数 + 1，最后一行为**空类**（条件丢弃时用）。
pub struct Film {
    pub g: Vec<f32>,
    pub b: Vec<f32>,
    pub nk: usize,
    pub c: usize,
}

impl Film {
    pub fn new(nk: usize, c: usize) -> Self {
        let rows = nk + 1;
        Film {
            g: vec![1.0; rows * c],
            b: vec![0.0; rows * c],
            nk,
            c,
        }
    }
    pub fn row(&self, class: Option<usize>) -> usize {
        class.filter(|&k| k < self.nk).unwrap_or(self.nk)
    }
    pub fn gamma(&self, class: Option<usize>) -> &[f32] {
        let r = self.row(class);
        &self.g[r * self.c..(r + 1) * self.c]
    }
    pub fn beta(&self, class: Option<usize>) -> &[f32] {
        let r = self.row(class);
        &self.b[r * self.c..(r + 1) * self.c]
    }
}

/// 全卷积栈：隐藏层（可选 BN）+ ReLU，最后一层 sigmoid。
pub struct Net {
    pub layers: Vec<Conv>,
    /// 与 `layers` 等长；只有隐藏层可能启用。
    pub bns: Vec<Option<Bn>>,
    /// 条件通道数（追加在输入后面，常数图）。
    pub cond: usize,
    /// 输出层是否线性（ε-预测需要无界输出；默认 sigmoid 给 x0 用）。
    pub linear_out: bool,
    /// 所有卷积层是否循环填充（可平铺纹理用）。
    pub circular: bool,
    /// 高通（拉普拉斯残差）匹配项的权重；0 = 关。线性输出模式下生效。
    pub highpass: f32,
    /// 每个隐层的 FiLM（`None` = 该层不做条件调制）。
    pub films: Vec<Option<Film>>,
    /// 类别数（0 = 未启用 FiLM）。
    pub nk: usize,
    /// FiLM 的学习率倍数（条件信号弱时需要放大；默认 1）。
    pub film_lr_mul: f32,
    /// 辅助分类头（`None` = 不加）。
    pub aux: Option<AuxCls>,
    /// 辅助分类损失的权重 λ。
    pub aux_w: f32,
    /// 诊断：上一批辅助头自己的 CE 与准确率。
    pub aux_ce: f32,
    pub aux_acc: f32,
    /// 每类一个**输出头**（共享主干）；`None` 的类走基础输出层。
    pub heads: Vec<Option<Conv>>,
    /// 方向性损失：(类别, 目标梯度角弧度, 权重)。`None` = 关。
    pub dir: Option<(usize, f32, f32)>,
    /// 噪声表（v-预测下用于把 v̂ 还原成 x̂0 供方向性损失使用）；空 = 不可用。
    pub abar: Vec<f32>,
    /// 结构张量矩匹配：`(类别, 目标 (Jxx,Jyy,Jxy), λ)`。`None` = 关。
    pub stm: Option<(usize, [f32; 3], f32)>,
    /// coherence 匹配：`(类别, 目标 c*, 目标总能量 T*, λ, 能量项权重)`。`None` = 关。
    pub coh: Option<(usize, f32, f32, f32, f32)>,
}

impl Net {
    pub fn new(cin: usize, hidden: &[usize], seed: u64) -> Self {
        let mut layers = Vec::new();
        let mut c = cin;
        for (i, &h) in hidden.iter().enumerate() {
            layers.push(Conv::new(c, h, seed + i as u64 * 7919));
            c = h;
        }
        layers.push(Conv::new(c, 1, seed + hidden.len() as u64 * 7919));
        let n = layers.len();
        // 默认不加 BN；`set_bn(true)` 时只给隐藏层
        let bns = (0..n).map(|_| None).collect();
        Net {
            layers,
            bns,
            cond: 0,
            linear_out: false,
            circular: false,
            highpass: 0.0,
            films: (0..n).map(|_| None).collect(),
            nk: 0,
            film_lr_mul: 1.0,
            aux: None,
            aux_w: 0.0,
            aux_ce: 0.0,
            aux_acc: 0.0,
            heads: Vec::new(),
            dir: None,
            abar: Vec::new(),
            stm: None,
            coh: None,
        }
    }

    /// 开「每类一个输出头」：共享主干，只多最后一层卷积（`nk` 个头）。
    pub fn set_heads(&mut self, nk: usize, seed: u64) {
        if self.layers.len() < 2 {
            return;
        }
        let cin = self.layers[self.layers.len() - 2].cout;
        self.heads = (0..nk)
            .map(|k| Some(Conv::new(cin, 1, seed + 1000 + k as u64)))
            .collect();
        // 输出头与基础输出层同源初值会打架，这里让基础层退居「空类」
    }

    /// 取该类应使用的输出层（没有专属头就走基础层）。
    fn out_layer(&self, class: Option<usize>) -> Option<&Conv> {
        class
            .and_then(|c| self.heads.get(c))
            .and_then(|h| h.as_ref())
    }

    /// 开辅助分类头：作用在**最后一层隐特征**的全局池化上。
    pub fn set_aux(&mut self, nk: usize) {
        if self.layers.len() < 2 {
            return;
        }
        let c = self.layers[self.layers.len() - 2].cout;
        self.aux = Some(AuxCls::new(nk, c));
    }

    /// 开关 FiLM 条件：`nk` 个类 + 1 个空类，只作用在隐藏层。
    pub fn set_film(&mut self, nk: usize) {
        self.nk = nk;
        let n = self.layers.len();
        for i in 0..n {
            self.films[i] = if i + 1 < n {
                Some(Film::new(nk, self.layers[i].cout))
            } else {
                None
            };
        }
    }

    /// 开关循环填充（可平铺纹理；零填充会让边界变平，接缝比 <1）。
    pub fn set_circular(&mut self, on: bool) {
        self.circular = on;
        for l in self.layers.iter_mut() {
            l.circular = on;
        }
    }

    /// 全局膨胀率（含输出头）：感受野 = 2·dil·层数 + 1，算力不变。
    pub fn set_dilation(&mut self, d: usize) {
        let d = d.max(1);
        for l in self.layers.iter_mut() {
            l.dil = d;
        }
        for h in self.heads.iter_mut().flatten() {
            h.dil = d;
        }
    }

    /// 开关 BN（隐藏层；最后一层不加）。
    pub fn set_bn(&mut self, on: bool) {
        let n = self.layers.len();
        for (i, b) in self.bns.iter_mut().enumerate() {
            *b = if on && i + 1 < n {
                Some(Bn::new(self.layers[i].cout))
            } else {
                None
            };
        }
    }

    fn input(&self, img: &[f32], cond: Option<usize>, w: usize, h: usize) -> Vec<Vec<f32>> {
        let mut x = vec![img.to_vec()];
        for k in 0..self.cond {
            x.push(vec![(cond == Some(k)) as u8 as f32; w * h]);
        }
        x
    }

    pub fn forward(&self, img: &[f32], cond: Option<usize>, w: usize, h: usize) -> Vec<f32> {
        let x = self.input(img, cond, w, h);
        self.forward_x(&x, w, h)
    }

    /// 直接吃一组通道图（扩散去噪器要把 `t/T` 当第二通道，`input` 造不出来）。
    pub fn forward_x(&self, x: &[Vec<f32>], w: usize, h: usize) -> Vec<f32> {
        self.forward_xc(x, None, w, h)
    }

    /// 带类别条件的前向（FiLM）。
    pub fn forward_xc(&self, x: &[Vec<f32>], class: Option<usize>, w: usize, h: usize) -> Vec<f32> {
        let mut x = x.to_vec();
        for i in 0..self.layers.len() {
            let mut y = if i + 1 == self.layers.len() {
                match self.out_layer(class) {
                    Some(hd) => hd.forward(&x, w, h),
                    None => self.layers[i].forward(&x, w, h),
                }
            } else {
                self.layers[i].forward(&x, w, h)
            };
            if let Some(f) = &self.films[i] {
                let (g, bb) = (f.gamma(class), f.beta(class));
                for (ch, v) in y.iter_mut().enumerate() {
                    let (gi, bi) = (g[ch], bb[ch]);
                    for t in v.iter_mut() {
                        *t = gi * *t + bi;
                    }
                }
            }
            if let Some(bn) = &self.bns[i] {
                // 推理也用**输入自身的统计**（单图 InstanceNorm 语义），
                // 否则训练（批统计）与评测（running stats）不一致，BN 版会直接崩。
                let mut b = bn.clone();
                y = b.forward(&y, w * h, true).0;
            }
            let last = i + 1 == self.layers.len();
            let linear = last && self.linear_out;
            for ch in y.iter_mut() {
                for v in ch.iter_mut() {
                    *v = if linear {
                        *v
                    } else if last {
                        1.0 / (1.0 + (-*v).exp())
                    } else {
                        v.max(0.0)
                    };
                }
            }
            x = y;
        }
        x.pop().unwrap()
    }

    /// 单张图训练一步（= batch size 1 的 `train_batch`）。
    pub fn train_step(
        &mut self,
        img: &[f32],
        target: &[f32],
        cond: Option<usize>,
        w: usize,
        h: usize,
        lr: f32,
    ) -> f32 {
        self.train_batch(&[(img.to_vec(), target.to_vec(), cond)], w, h, lr, false)
    }

    /// 多图梯度累积 + 批量更新（`adam=true` 用 Adam）。返回平均 BCE。
    pub fn train_batch(
        &mut self,
        batch: &[(Vec<f32>, Vec<f32>, Option<usize>)],
        w: usize,
        h: usize,
        lr: f32,
        adam: bool,
    ) -> f32 {
        let maps: Vec<(Vec<Vec<f32>>, Vec<f32>)> = batch
            .iter()
            .map(|(img, tgt, cond)| (self.input(img, *cond, w, h), tgt.clone()))
            .collect();
        self.train_batch_x(&maps, w, h, lr, adam)
    }

    /// 多通道输入版的批量训练：`batch[i] = (输入通道图, 目标图)`。
    pub fn train_batch_x(
        &mut self,
        batch: &[(Vec<Vec<f32>>, Vec<f32>)],
        w: usize,
        h: usize,
        lr: f32,
        adam: bool,
    ) -> f32 {
        let with_class: Vec<Sample> = batch
            .iter()
            .map(|(m, t)| (m.clone(), t.clone(), None))
            .collect();
        self.train_batch_xc(&with_class, w, h, lr, adam)
    }

    /// 带类别条件的批量训练（FiLM）：`batch[i] = (输入通道图, 目标图, 类别)`。
    pub fn train_batch_xc(
        &mut self,
        batch: &[Sample],
        w: usize,
        h: usize,
        lr: f32,
        adam: bool,
    ) -> f32 {
        let n = w * h;
        let k = batch.len().max(1);
        let mut acc_w: Vec<Vec<f32>> = self.layers.iter().map(|l| vec![0.0; l.w.len()]).collect();
        let mut acc_b: Vec<Vec<f32>> = self.layers.iter().map(|l| vec![0.0; l.b.len()]).collect();
        let mut acc_sc: Vec<Vec<f32>> = self
            .layers
            .iter()
            .map(|l| vec![0.0; l.scale.len()])
            .collect();
        let mut acc_g: Vec<Option<Vec<f32>>> = self
            .bns
            .iter()
            .map(|b| b.as_ref().map(|bb| vec![0.0; bb.gamma.len()]))
            .collect();
        let mut acc_be: Vec<Option<Vec<f32>>> = self
            .bns
            .iter()
            .map(|b| b.as_ref().map(|bb| vec![0.0; bb.beta.len()]))
            .collect();
        let mut acc_hw: Vec<Option<Vec<f32>>> = self
            .heads
            .iter()
            .map(|h| h.as_ref().map(|c| vec![0.0; c.w.len()]))
            .collect();
        let mut acc_hb: Vec<Option<Vec<f32>>> = self
            .heads
            .iter()
            .map(|h| h.as_ref().map(|c| vec![0.0; c.b.len()]))
            .collect();
        let mut acc_hsc: Vec<Option<Vec<f32>>> = self
            .heads
            .iter()
            .map(|h| h.as_ref().map(|c| vec![0.0; c.scale.len()]))
            .collect();
        let (mut aux_ce_acc, mut aux_hit, mut aux_cnt) = (0.0f32, 0usize, 0usize);
        // 辅助分类头的梯度（w: nk×c，b: nk）
        let mut acc_aw: Vec<f32> = self
            .aux
            .as_ref()
            .map(|a| vec![0.0; a.w.len()])
            .unwrap_or_default();
        let mut acc_ab: Vec<f32> = self
            .aux
            .as_ref()
            .map(|a| vec![0.0; a.b.len()])
            .unwrap_or_default();
        // FiLM 的梯度按「层 × 类 × 通道」累加
        let mut acc_fg: Vec<Option<Vec<f32>>> = self
            .films
            .iter()
            .map(|f| f.as_ref().map(|ff| vec![0.0; ff.g.len()]))
            .collect();
        let mut acc_fb: Vec<Option<Vec<f32>>> = self
            .films
            .iter()
            .map(|f| f.as_ref().map(|ff| vec![0.0; ff.b.len()]))
            .collect();
        let mut loss = 0.0f64;
        for (maps_in, target, class) in batch {
            let mut ins: Vec<Vec<Vec<f32>>> = Vec::new();
            let mut pre_relu: Vec<Vec<Vec<f32>>> = Vec::new();
            let mut xhats: Vec<Option<Vec<Vec<f32>>>> = Vec::new();
            // 每层的 (FiLM 前的 conv 输出, 该类的 γ, 类别)
            let mut film_state: Vec<Option<FilmState>> = Vec::new();
            let mut x = maps_in.clone();
            for li in 0..self.layers.len() {
                let mut y = if li + 1 == self.layers.len() {
                    match self.out_layer(*class) {
                        Some(hd) => hd.forward(&x, w, h),
                        None => self.layers[li].forward(&x, w, h),
                    }
                } else {
                    self.layers[li].forward(&x, w, h)
                };
                let mut fs = None;
                if let Some(f) = &self.films[li] {
                    let pre = y.clone();
                    let g = f.gamma(*class).to_vec();
                    let b = f.beta(*class);
                    for (ch, v) in y.iter_mut().enumerate() {
                        let (gi, bi) = (g[ch], b[ch]);
                        for t in v.iter_mut() {
                            *t = gi * *t + bi;
                        }
                    }
                    fs = Some((pre, g, *class));
                }
                film_state.push(fs);
                let mut xh = None;
                if let Some(bn) = self.bns[li].as_mut() {
                    let (z, xx) = bn.forward(&y, n, true);
                    y = z;
                    xh = Some(xx);
                }
                ins.push(x.clone());
                pre_relu.push(y.clone());
                xhats.push(xh);
                let last = li + 1 == self.layers.len();
                let linear = last && self.linear_out;
                for ch in y.iter_mut() {
                    for v in ch.iter_mut() {
                        *v = if linear {
                            *v
                        } else if last {
                            1.0 / (1.0 + (-*v).exp())
                        } else {
                            v.max(0.0)
                        };
                    }
                }
                x = y;
            }
            // 辅助分类：对「最后一层隐特征」做全局平均池化
            let mut aux_dpool: Option<Vec<f32>> = None;
            let mut aux_feat: Option<&Vec<Vec<f32>>> = None;
            // 只在低噪声段加分类损失：高 t 时输入近纯噪声，类别本质不可辨，硬加只会浪费梯度
            let t_frac = maps_in.get(1).map(|m| m[0]).unwrap_or(0.0);
            let aux_scale = self.aux_w * (1.0 - t_frac).powi(2);
            if let (Some(a), Some(cls)) = (self.aux.as_ref(), class)
                && aux_scale > 1e-4
                && *cls < a.nk
                && self.layers.len() >= 2
            {
                {
                    let feats = &ins[self.layers.len() - 1];
                    let mut pool = vec![0.0f32; a.c];
                    for (ch, v) in feats.iter().enumerate() {
                        pool[ch] = v.iter().sum::<f32>() / n as f32;
                    }
                    let logits = a.logits(&pool);
                    let mx = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                    let exps: Vec<f32> = logits.iter().map(|v| (v - mx).exp()).collect();
                    let z: f32 = exps.iter().sum::<f32>().max(1e-9);
                    let probs: Vec<f32> = exps.iter().map(|v| v / z).collect();
                    loss += f64::from(
                        aux_scale * -(probs.get(*cls).copied().unwrap_or(1e-6).max(1e-6).ln()),
                    );
                    let mut dlog = probs.clone();
                    dlog[*cls] -= 1.0;
                    for k in 0..a.nk {
                        acc_ab[k] += aux_scale * dlog[k];
                        for i in 0..a.c {
                            acc_aw[k * a.c + i] += aux_scale * dlog[k] * pool[i];
                        }
                    }
                    let mut dpool = vec![0.0f32; a.c];
                    for (i, dp) in dpool.iter_mut().enumerate() {
                        *dp = (0..a.nk)
                            .map(|k| aux_scale * dlog[k] * a.w[k * a.c + i])
                            .sum();
                    }
                    aux_ce_acc += -probs.get(*cls).copied().unwrap_or(1e-6).max(1e-6).ln();
                    let best = probs
                        .iter()
                        .enumerate()
                        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    if best == *cls {
                        aux_hit += 1;
                    }
                    aux_cnt += 1;
                    aux_dpool = Some(dpool);
                    aux_feat = Some(feats);
                }
            }

            let pred = &x[0];
            let mut d = vec![vec![0.0f32; n]; 1];
            for i in 0..n {
                let t = target[i];
                if self.linear_out {
                    // ε/v-预测：平方误差（输出无界）
                    let e = pred[i] - t;
                    loss += 0.5 * f64::from(e * e);
                    d[0][i] = e / n as f32 / k as f32;
                } else {
                    let p = pred[i].clamp(1e-6, 1.0 - 1e-6);
                    loss -= f64::from(t * p.ln() + (1.0 - t) * (1.0 - p).ln());
                    d[0][i] = (p - t) / n as f32 / k as f32; // 批内平均
                }
            }
            // coherence 匹配：c = sqrt((Jxx−Jyy)²+4Jxy²)/(Jxx+Jyy)，另加弱能量项防止退化成光栅/空图
            if let (Some((ccls, cstar, tstar, lam, w_t)), Some(cls)) = (self.coh, class)
                && *cls == ccls
                && lam > 0.0
                && self.linear_out
                && !self.abar.is_empty()
            {
                let tmax = self.abar.len().saturating_sub(1).max(1);
                let idx = ((t_frac * tmax as f32).round() as usize).min(tmax);
                let a = self.abar[idx].clamp(0.0, 1.0);
                let (sa, se) = (a.sqrt(), (1.0 - a).sqrt());
                let xt = &maps_in[0];
                let x0: Vec<f32> = (0..n).map(|i| sa * xt[i] - se * pred[i]).collect();
                let (lval, dx0) = coh_loss_grad(&x0, w, h, cstar, tstar, lam, w_t);
                loss += f64::from(lval);
                for (i, g) in dx0.iter().enumerate() {
                    d[0][i] += -se * g / k as f32;
                }
            }

            // 结构张量矩匹配：把 x̂0 的 (Jxx,Jyy,Jxy) 拉向该类真实值（同时约束各向异性与梯度能量）
            if let (Some((scls, jt, lam)), Some(cls)) = (self.stm, class)
                && *cls == scls
                && lam > 0.0
                && self.linear_out
                && !self.abar.is_empty()
            {
                let tmax = self.abar.len().saturating_sub(1).max(1);
                let idx = ((t_frac * tmax as f32).round() as usize).min(tmax);
                let a = self.abar[idx].clamp(0.0, 1.0);
                let (sa, se) = (a.sqrt(), (1.0 - a).sqrt());
                let xt = &maps_in[0];
                let x0: Vec<f32> = (0..n).map(|i| sa * xt[i] - se * pred[i]).collect();
                let (gx, gy) = sobel(&x0, w, h);
                let inner = (w.saturating_sub(2)) * (h.saturating_sub(2));
                let (mut jxx, mut jyy, mut jxy) = (0.0f32, 0.0f32, 0.0f32);
                for i in 0..n {
                    jxx += gx[i] * gx[i];
                    jyy += gy[i] * gy[i];
                    jxy += gx[i] * gy[i];
                }
                let inv = 1.0 / inner.max(1) as f32;
                let (jxx, jyy, jxy) = (jxx * inv, jyy * inv, jxy * inv);
                // 相对误差（用目标总能量做尺度），避免大 λ 直接爆
                let sc = (jt[0] + jt[1]).max(1e-6);
                let (ex, ey, exy) = ((jxx - jt[0]) / sc, (jyy - jt[1]) / sc, (jxy - jt[2]) / sc);
                loss += f64::from(lam * (ex * ex + ey * ey + 2.0 * exy * exy));
                // dL/dg：链式（J 对 g 的导数 × 误差项）
                let cxx = 2.0 * lam * ex / sc * inv;
                let cyy = 2.0 * lam * ey / sc * inv;
                let cxy = 4.0 * lam * exy / sc * inv;
                let gx2: Vec<f32> = (0..n).map(|i| 2.0 * (cxx * gx[i] + cxy * gy[i])).collect();
                let gy2: Vec<f32> = (0..n).map(|i| 2.0 * (cyy * gy[i] + cxy * gx[i])).collect();
                let dx0 = sobel_adjoint(&gx2, &gy2, w, h);
                for (i, g) in dx0.iter().enumerate() {
                    d[0][i] += -se * g / k as f32;
                }
            }

            // 方向性损失：抑制「垂直于目标主方向」的梯度分量（对 x̂0 施加）
            if let (Some((dcls, angle, lam)), Some(cls)) = (self.dir, class)
                && *cls == dcls
                && lam > 0.0
                && self.linear_out
                && !self.abar.is_empty()
            {
                {
                    let tmax = self.abar.len().saturating_sub(1).max(1);
                    let idx = ((t_frac * tmax as f32).round() as usize).min(tmax);
                    let a = self.abar[idx].clamp(0.0, 1.0);
                    let (sa, se) = (a.sqrt(), (1.0 - a).sqrt());
                    let xt = &maps_in[0];
                    let x0: Vec<f32> = (0..n).map(|i| sa * xt[i] - se * pred[i]).collect();
                    let (gx, gy) = sobel(&x0, w, h);
                    // m ⊥ 目标梯度方向 n=(cosθ, sinθ)
                    let (mx, my) = (-angle.sin(), angle.cos());
                    let mut gsum = 0.0f64;
                    let mut coef = vec![0.0f32; n];
                    let inner = (w.saturating_sub(2)) * (h.saturating_sub(2));
                    for i in 0..n {
                        let gm = gx[i] * mx + gy[i] * my;
                        gsum += f64::from(gm * gm);
                        coef[i] = 2.0 * lam * gm / inner.max(1) as f32;
                    }
                    loss += f64::from(lam) * gsum / inner.max(1) as f64;
                    let gx2: Vec<f32> = coef.iter().map(|c| c * mx).collect();
                    let gy2: Vec<f32> = coef.iter().map(|c| c * my).collect();
                    let dx0 = sobel_adjoint(&gx2, &gy2, w, h);
                    for (i, g) in dx0.iter().enumerate() {
                        d[0][i] += -se * g / k as f32;
                    }
                }
            }

            // 高通匹配：对残差做 4 邻域拉普拉斯，边界跳过（保证算子自伴，梯度就是它自己）
            if self.linear_out && self.highpass > 0.0 && w > 2 && h > 2 {
                let mut lap = vec![0.0f32; n];
                for yy in 1..h - 1 {
                    for xx in 1..w - 1 {
                        let i = yy * w + xx;
                        let r = |j: usize| pred[j] - target[j];
                        lap[i] = r(i) - 0.25 * (r(i - 1) + r(i + 1) + r(i - w) + r(i + w));
                    }
                }
                for yy in 1..h - 1 {
                    for xx in 1..w - 1 {
                        let i = yy * w + xx;
                        let l = lap[i];
                        loss += f64::from(self.highpass * 0.5 * l * l);
                        let g = self.highpass * l / n as f32 / k as f32;
                        d[0][i] += g;
                        d[0][i - 1] -= g * 0.25;
                        d[0][i + 1] -= g * 0.25;
                        d[0][i - w] -= g * 0.25;
                        d[0][i + w] -= g * 0.25;
                    }
                }
            }
            for li in (0..self.layers.len()).rev() {
                if li + 1 < self.layers.len() {
                    for c in 0..d.len() {
                        for i in 0..n {
                            if pre_relu[li][c][i] <= 0.0 {
                                d[c][i] = 0.0;
                            }
                        }
                    }
                }
                if let (Some(bn), Some(xh)) = (self.bns[li].as_ref(), xhats[li].as_ref()) {
                    let (dx, dg, db) = bn.backward(xh, &d, n);
                    if let Some(ag) = acc_g[li].as_mut() {
                        for (a, g) in ag.iter_mut().zip(&dg) {
                            *a += g;
                        }
                    }
                    if let Some(ab) = acc_be[li].as_mut() {
                        for (a, g) in ab.iter_mut().zip(&db) {
                            *a += g;
                        }
                    }
                    d = dx;
                }
                // 辅助分类头对「该层激活输出」的梯度：只在 ReLU 未截断处有效
                if li + 2 == self.layers.len()
                    && let (Some(dpool), Some(feats)) = (aux_dpool.as_ref(), aux_feat.as_ref())
                {
                    for (ch, row) in feats.iter().enumerate() {
                        let gg = dpool[ch] / n as f32;
                        for (i, v) in row.iter().enumerate() {
                            if *v > 0.0 {
                                d[ch][i] += gg;
                            }
                        }
                    }
                }
                if let (Some((pre, g, cls)), Some(ag), Some(ab)) =
                    (&film_state[li], acc_fg[li].as_mut(), acc_fb[li].as_mut())
                {
                    {
                        let f = self.films[li].as_ref().unwrap();
                        let r = f.row(*cls);
                        let (lo, hi) = (r * f.c, (r + 1) * f.c);
                        for ch in 0..d.len() {
                            let mut sg = 0.0f32;
                            let mut sb = 0.0f32;
                            for i in 0..n {
                                sg += d[ch][i] * pre[ch][i];
                                sb += d[ch][i];
                            }
                            ag[lo + ch] += sg;
                            ab[lo + ch] += sb;
                        }
                        // dL/dpre = γ · dL/dy
                        for (ch, row) in d.iter_mut().enumerate() {
                            let gi = g[ch];
                            for v in row.iter_mut() {
                                *v *= gi;
                            }
                        }
                        let _ = (lo, hi);
                    }
                }
                let head_hit = if li + 1 == self.layers.len()
                    && let Some(c) = *class
                    && let Some(hd) = self.heads.get(c).and_then(|h| h.as_ref())
                {
                    let (dx, dw, db, dsc) = hd.backward(&ins[li], w, h, &d);
                    if let (Some(aw), Some(ab), Some(asc)) =
                        (acc_hw[c].as_mut(), acc_hb[c].as_mut(), acc_hsc[c].as_mut())
                    {
                        for (a, g) in aw.iter_mut().zip(&dw) {
                            *a += g;
                        }
                        for (a, g) in ab.iter_mut().zip(&db) {
                            *a += g;
                        }
                        for (a, g) in asc.iter_mut().zip(&dsc) {
                            *a += g;
                        }
                    }
                    Some(dx)
                } else {
                    None
                };
                if let Some(dx) = head_hit {
                    d = dx;
                    continue;
                }
                let (dx, dw, db, dsc) = self.layers[li].backward(&ins[li], w, h, &d);
                for (a, g) in acc_w[li].iter_mut().zip(&dw) {
                    *a += g;
                }
                for (a, g) in acc_b[li].iter_mut().zip(&db) {
                    *a += g;
                }
                for (a, g) in acc_sc[li].iter_mut().zip(&dsc) {
                    *a += g;
                }
                d = dx;
            }
        }
        if aux_cnt > 0 {
            self.aux_ce = aux_ce_acc / aux_cnt as f32;
            self.aux_acc = aux_hit as f32 / aux_cnt as f32;
        }
        for c in 0..self.heads.len() {
            if let (Some(hd), Some(aw), Some(ab), Some(asc)) = (
                self.heads[c].as_mut(),
                acc_hw[c].as_ref(),
                acc_hb[c].as_ref(),
                acc_hsc[c].as_ref(),
            ) {
                hd.apply(aw, ab, asc, lr, adam);
            }
        }
        if let Some(a) = self.aux.as_mut() {
            for (w, g) in a.w.iter_mut().zip(&acc_aw) {
                *w -= lr * g / k as f32;
            }
            for (b, g) in a.b.iter_mut().zip(&acc_ab) {
                *b -= lr * g / k as f32;
            }
        }
        for li in 0..self.layers.len() {
            if let (Some(f), Some(ag), Some(ab)) = (
                self.films[li].as_mut(),
                acc_fg[li].as_ref(),
                acc_fb[li].as_ref(),
            ) {
                let rows = f.nk + 1;
                for r in 0..rows {
                    let (lo, hi) = (r * f.c, (r + 1) * f.c);
                    let dg: Vec<f32> = ag[lo..hi].iter().map(|v| v / k as f32).collect();
                    let db: Vec<f32> = ab[lo..hi].iter().map(|v| v / k as f32).collect();
                    // 按行更新：复用 apply_step 的行定位（用 0..nk 隐式映射）
                    let flr = lr * self.film_lr_mul;
                    for i in 0..f.c {
                        f.g[lo + i] = (f.g[lo + i] - flr * dg[i]).clamp(1e-3, 10.0);
                        f.b[lo + i] -= flr * db[i];
                    }
                }
            }
            self.layers[li].apply(&acc_w[li], &acc_b[li], &acc_sc[li], lr, adam);
            if let (Some(bn), Some(g), Some(be)) = (
                self.bns[li].as_mut(),
                acc_g[li].as_ref(),
                acc_be[li].as_ref(),
            ) {
                bn.apply(g, be, lr * 0.1, adam);
            }
        }
        (loss / k as f64) as f32
    }
}

impl Net {
    /// 存盘（文本，`TPC1`）：层维度 + 权重 + bias + scale，再跟 BN 的 γ/β。
    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        use std::fmt::Write as _;
        let mut out = String::new();
        let _ = writeln!(
            out,
            "TPC7 {} {} {} {} {} {} {} {}",
            self.layers.len(),
            self.cond,
            self.linear_out as u8,
            self.circular as u8,
            self.highpass,
            self.nk,
            self.heads.len(),
            self.layers.first().map(|l| l.dil).unwrap_or(1)
        );
        for l in &self.layers {
            let _ = writeln!(out, "{} {}", l.cin, l.cout);
            for v in [&l.w, &l.b, &l.scale] {
                for x in v.iter() {
                    let _ = write!(out, "{x} ");
                }
                let _ = writeln!(out);
            }
        }
        let nbn = self.bns.iter().filter(|b| b.is_some()).count();
        let _ = writeln!(out, "BN {nbn}");
        for i in 0..self.layers.len() {
            match &self.bns[i] {
                Some(b) => {
                    let _ = writeln!(out, "{i} 1");
                    for v in [&b.gamma, &b.beta] {
                        for x in v.iter() {
                            let _ = write!(out, "{x} ");
                        }
                        let _ = writeln!(out);
                    }
                }
                None => {
                    let _ = writeln!(out, "{i} 0");
                }
            }
        }
        // FiLM：层号 + 是否启用 + (nk+1)×c 的 g/b
        let nfilm = self.films.iter().filter(|f| f.is_some()).count();
        let _ = writeln!(out, "FILM {nfilm}");
        for (i, f) in self.films.iter().enumerate() {
            match f {
                Some(ff) => {
                    let _ = writeln!(out, "{i} {} {}", ff.nk, ff.c);
                    for v in [&ff.g, &ff.b] {
                        for x in v.iter() {
                            let _ = write!(out, "{x} ");
                        }
                        let _ = writeln!(out);
                    }
                }
                None => {
                    let _ = writeln!(out, "{i} 0 0");
                }
            }
        }
        // 输出头：数量 + 每个头（启用=1 + cin/cout + w/b/scale）
        let _ = writeln!(out, "HEADS {}", self.heads.len());
        for h in &self.heads {
            match h {
                Some(c) => {
                    let _ = writeln!(out, "1 {} {}", c.cin, c.cout);
                    for v in [&c.w, &c.b, &c.scale] {
                        for x in v.iter() {
                            let _ = write!(out, "{x} ");
                        }
                        let _ = writeln!(out);
                    }
                }
                None => {
                    let _ = writeln!(out, "0 0 0");
                }
            }
        }
        std::fs::write(path, out).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let txt = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        fn take<'a>(
            it: &mut impl Iterator<Item = &'a str>,
            path: &std::path::Path,
            what: &str,
        ) -> Result<f32, String> {
            it.next()
                .ok_or_else(|| format!("{}: 缺 {what}", path.display()))?
                .parse::<f32>()
                .map_err(|e| format!("{}: {what} 解析失败 {e}", path.display()))
        }
        let mut it = txt.split_whitespace();
        let head = it.next();
        if !matches!(
            head,
            Some("TPC1")
                | Some("TPC2")
                | Some("TPC3")
                | Some("TPC4")
                | Some("TPC5")
                | Some("TPC6")
                | Some("TPC7")
        ) {
            return Err(format!("{}: 不是 TPC1..TPC7", path.display()));
        }
        let nl = take(&mut it, path, "层数")? as usize;
        let cond = take(&mut it, path, "cond")? as usize;
        let has_linear = matches!(
            head,
            Some("TPC2") | Some("TPC3") | Some("TPC4") | Some("TPC5") | Some("TPC6") | Some("TPC7")
        );
        let linear_out = has_linear && take(&mut it, path, "linear")? != 0.0;
        let has_circ = matches!(
            head,
            Some("TPC3") | Some("TPC4") | Some("TPC5") | Some("TPC6") | Some("TPC7")
        );
        let circular = has_circ && take(&mut it, path, "circular")? != 0.0;
        let has_hp = matches!(
            head,
            Some("TPC4") | Some("TPC5") | Some("TPC6") | Some("TPC7")
        );
        let highpass = if has_hp {
            take(&mut it, path, "highpass")?
        } else {
            0.0
        };
        let nk_hdr = if matches!(head, Some("TPC5") | Some("TPC6") | Some("TPC7")) {
            take(&mut it, path, "nk")? as usize
        } else {
            0
        };
        let nheads = if matches!(head, Some("TPC6") | Some("TPC7")) {
            take(&mut it, path, "nheads")? as usize
        } else {
            0
        };
        let dil = if head == Some("TPC7") {
            (take(&mut it, path, "dilation")? as usize).max(1)
        } else {
            1
        };
        let mut layers = Vec::with_capacity(nl);
        for _ in 0..nl {
            let cin = take(&mut it, path, "cin")? as usize;
            let cout = take(&mut it, path, "cout")? as usize;
            let mut l = Conv::new(cin, cout, 1);
            for v in l.w.iter_mut() {
                *v = take(&mut it, path, "w")?;
            }
            for v in l.b.iter_mut() {
                *v = take(&mut it, path, "b")?;
            }
            for v in l.scale.iter_mut() {
                *v = take(&mut it, path, "scale")?;
            }
            layers.push(l);
        }
        let mut bns: Vec<Option<Bn>> = (0..nl).map(|_| None).collect();
        if it.next() != Some("BN") {
            return Err(format!("{}: 缺 BN 段", path.display()));
        }
        let _n = take(&mut it, path, "BN 数量")? as usize;
        for _ in 0..nl {
            let idx = take(&mut it, path, "BN 层号")? as usize;
            let on = take(&mut it, path, "BN 开关")? != 0.0;
            if !on {
                continue;
            }
            let c = layers[idx].cout;
            let mut b = Bn::new(c);
            for v in b.gamma.iter_mut() {
                *v = take(&mut it, path, "gamma")?;
            }
            for v in b.beta.iter_mut() {
                *v = take(&mut it, path, "beta")?;
            }
            bns[idx] = Some(b);
        }
        let mut net = Net {
            layers,
            bns,
            cond,
            linear_out,
            circular: false,
            highpass,
            films: (0..nl).map(|_| None).collect(),
            nk: 0,
            film_lr_mul: 1.0,
            aux: None,
            aux_w: 0.0,
            aux_ce: 0.0,
            aux_acc: 0.0,
            heads: Vec::new(),
            dir: None,
            abar: Vec::new(),
            stm: None,
            coh: None,
        };
        net.set_circular(circular);
        if nk_hdr > 0 {
            net.set_film(nk_hdr);
            if it.next() != Some("FILM") {
                return Err(format!("{}: 缺 FILM 段", path.display()));
            }
            let _n = take(&mut it, path, "FILM 数量")? as usize;
            for _ in 0..nl {
                let idx = take(&mut it, path, "Film 层号")? as usize;
                let k = take(&mut it, path, "Film nk")? as usize;
                let c = take(&mut it, path, "Film c")? as usize;
                if k == 0 || c == 0 {
                    continue;
                }
                if let Some(f) = net.films[idx].as_mut() {
                    for v in f.g.iter_mut() {
                        *v = take(&mut it, path, "Film g")?;
                    }
                    for v in f.b.iter_mut() {
                        *v = take(&mut it, path, "Film b")?;
                    }
                }
            }
        }
        if nheads > 0 {
            if it.next() != Some("HEADS") {
                return Err(format!("{}: 缺 HEADS 段", path.display()));
            }
            let _n = take(&mut it, path, "HEADS 数量")? as usize;
            net.heads = Vec::with_capacity(nheads);
            for _ in 0..nheads {
                let on = take(&mut it, path, "head on")? != 0.0;
                let cin = take(&mut it, path, "head cin")? as usize;
                let cout = take(&mut it, path, "head cout")? as usize;
                if !on {
                    net.heads.push(None);
                    continue;
                }
                let mut c = Conv::new(cin, cout, 1);
                for v in c.w.iter_mut() {
                    *v = take(&mut it, path, "head w")?;
                }
                for v in c.b.iter_mut() {
                    *v = take(&mut it, path, "head b")?;
                }
                for v in c.scale.iter_mut() {
                    *v = take(&mut it, path, "head scale")?;
                }
                net.heads.push(Some(c));
            }
        }
        net.set_dilation(dil);
        Ok(net)
    }
}

/// v-预测适配：`v ≡ √ᾱ·ε − √(1−ᾱ)·x0`，换算 `x̂0 = √ᾱ·x_t − √(1−ᾱ)·v̂`。
/// 高 t（ᾱ→0）时 x̂0 ≈ −v̂，**没有 1/√ᾱ**，这条参数化就是为稳定而生的。
pub struct VDen {
    pub net: Net,
    pub abar: Vec<f32>,
    /// 追加在 `[x_t, t/T]` 之后的常数条件图（如目标均值/对比）；空 = 无条件。
    pub extra: Vec<f32>,
    /// 是否把 x̂0 裁到 [0,1]；归一化（z-score）训练时必须为 false。
    pub clamp: bool,
    /// FiLM 的类别 id（`None` = 空类）。
    pub class: Option<usize>,
}

impl crate::diffuse::Denoiser for VDen {
    fn predict_x0(
        &self,
        xt: &[f32],
        t_frac: f32,
        cond: Option<usize>,
        w: usize,
        h: usize,
    ) -> Vec<f32> {
        let mut maps = vec![xt.to_vec(), vec![t_frac; w * h]];
        for e in &self.extra {
            maps.push(vec![*e; w * h]);
        }
        for k in 0..self.net.cond {
            maps.push(vec![(cond == Some(k)) as u8 as f32; w * h]);
        }
        let v = self.net.forward_xc(&maps, self.class, w, h);
        let tmax = self.abar.len().saturating_sub(1).max(1);
        let idx = ((t_frac * tmax as f32).round() as usize).min(tmax);
        let a = self.abar[idx].clamp(0.0, 1.0);
        let (sa, se) = (a.sqrt(), (1.0 - a).sqrt());
        xt.iter()
            .zip(&v)
            .map(|(x, vv)| {
                let x0 = sa * x - se * vv;
                if self.clamp { x0.clamp(0.0, 1.0) } else { x0 }
            })
            .collect()
    }

    fn input_dim(&self) -> usize {
        2 + self.net.cond
    }
}

/// ε-空间 DDIM（数值稳定）：**不显式构造 x̂0**，避免高 t 时除以 √ᾱ 爆炸。
///   x_{t−1} = √(ᾱ_{t−1}/ᾱ_t)·x_t + [√(1−ᾱ_{t−1}) − √(ᾱ_{t−1}/ᾱ_t)·√(1−ᾱ_t)]·ε̂
/// 最后一步（t=0）才换算 x̂0 并裁到 0..1。
pub fn ddim_sample_eps(
    net: &Net,
    abar: &[f32],
    w: usize,
    h: usize,
    seed: u64,
    eps_scale: f32,
) -> Vec<f32> {
    let tmax = abar.len().saturating_sub(1).max(1);
    let mut x = crate::diffuse::initial_noise(w * h, seed);
    for t in (0..tmax).rev() {
        let (at, atp) = (abar[t + 1].max(1e-8), abar[t].max(1e-8));
        let tf = (t + 1) as f32 / tmax as f32;
        let mut maps = vec![x.clone(), vec![tf; w * h]];
        for k in 0..net.cond {
            maps.push(vec![0.0; w * h]);
            let _ = k;
        }
        let mut e = net.forward_x(&maps, w, h);
        if (eps_scale - 1.0).abs() > 1e-6 {
            for v in e.iter_mut() {
                *v *= eps_scale;
            }
        }
        let ratio = (atp / at).sqrt();
        let coef = (1.0 - atp).sqrt() - ratio * (1.0 - at).sqrt();
        if t == 0 {
            let sa = at.sqrt();
            let se = (1.0 - at).sqrt();
            return x
                .iter()
                .zip(&e)
                .map(|(xt, ee)| ((xt - se * ee) / sa).clamp(0.0, 1.0))
                .collect();
        }
        x = x
            .iter()
            .zip(&e)
            .map(|(xt, ee)| ratio * xt + coef * ee)
            .collect();
    }
    x.iter().map(|v| v.clamp(0.0, 1.0)).collect()
}

/// ε-预测网络的 DDIM 适配：内部算 ε̂，再换算成 `x̂0 = (x_t − √(1−ᾱ)·ε̂)/√ᾱ`。
/// 为什么需要它：x0-预测的目标是条件均值，细纹理会被平均掉；ε 不含这个偏置。
pub struct EpsDen {
    pub net: Net,
    pub abar: Vec<f32>,
    /// ε 的温度系数：<1 抑制过冲（推断期旋钮，不用重训）。
    pub eps_scale: f32,
}

impl crate::diffuse::Denoiser for EpsDen {
    fn predict_x0(
        &self,
        xt: &[f32],
        t_frac: f32,
        cond: Option<usize>,
        w: usize,
        h: usize,
    ) -> Vec<f32> {
        let mut maps = vec![xt.to_vec(), vec![t_frac; w * h]];
        for k in 0..self.net.cond {
            maps.push(vec![(cond == Some(k)) as u8 as f32; w * h]);
        }
        let mut eps = self.net.forward_x(&maps, w, h);
        if (self.eps_scale - 1.0).abs() > 1e-6 {
            for e in eps.iter_mut() {
                *e *= self.eps_scale;
            }
        }
        let tmax = self.abar.len().saturating_sub(1).max(1);
        let idx = ((t_frac * tmax as f32).round() as usize).min(tmax);
        let a = self.abar[idx].max(1e-4);
        let (sa, se) = (a.sqrt(), (1.0 - a).sqrt());
        xt.iter()
            .zip(&eps)
            .map(|(x, e)| ((x - se * e) / sa).clamp(0.0, 1.0))
            .collect()
    }

    fn input_dim(&self) -> usize {
        2 + self.net.cond
    }
}

/// 当扩散去噪器用：输入通道 = `[x_t, t/T, 条件 one-hot…]`（`Net::new(2 + cond, …)`）。
impl crate::diffuse::Denoiser for Net {
    fn predict_x0(
        &self,
        xt: &[f32],
        t_frac: f32,
        cond: Option<usize>,
        w: usize,
        h: usize,
    ) -> Vec<f32> {
        let mut maps = vec![xt.to_vec(), vec![t_frac; w * h]];
        for k in 0..self.cond {
            maps.push(vec![(cond == Some(k)) as u8 as f32; w * h]);
        }
        self.forward_x(&maps, w, h)
    }

    fn input_dim(&self) -> usize {
        2 + self.cond
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conv_forward_shape_and_zero_pad() {
        let c = Conv::new(1, 2, 1);
        let x = vec![vec![1.0f32; 16]];
        let y = c.forward(&x, 4, 4);
        assert_eq!(y.len(), 2);
        assert_eq!(y[0].len(), 16);
    }

    #[test]
    fn learns_constant_target() {
        // 最小诊断：输入常数 0.9、目标常数 0.9，单层网络应把输出推到 0.9
        let (w, h) = (6usize, 6usize);
        let img = vec![0.9f32; w * h];
        let tgt = vec![0.9f32; w * h];
        let mut net = Net::new(1, &[], 3);
        for _ in 0..3000 {
            net.train_step(&img, &tgt, None, w, h, 0.5);
        }
        let p = net.forward(&img, None, w, h);
        let mean = p.iter().sum::<f32>() / p.len() as f32;
        assert!(mean > 0.85, "应学成 0.9，实际 {mean:.3}");
    }

    #[test]
    fn net_save_load_roundtrip() {
        let mut net = Net::new(2, &[4, 3], 11);
        net.set_bn(true);
        net.set_dilation(2);
        let (w, h) = (5usize, 5usize);
        let x = vec![vec![0.4f32; w * h], vec![0.25f32; w * h]];
        let a = net.forward_x(&x, w, h);
        let p = std::env::temp_dir().join("tconv_roundtrip.tpc");
        net.save(&p).unwrap();
        let back = Net::load(&p).unwrap();
        assert_eq!(back.layers[0].dil, 2, "膨胀率必须存进文件");
        let b = back.forward_x(&x, w, h);
        for (p, q) in a.iter().zip(&b) {
            assert!((p - q).abs() < 1e-5, "存读应逐位一致 {p} vs {q}");
        }
        assert_eq!(back.cond, 0);
        assert!(!back.linear_out);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn dilation_widens_the_tap_spacing() {
        // 单通道、单输出：输入中心放一个脉冲，dil=2 时只有 ±2 的抽头看得到它
        let mut c = Conv::new(1, 1, 5);
        c.ternary = false;
        c.w.fill(1.0);
        c.b.fill(0.0);
        c.scale.fill(1.0);
        let (w, h) = (7usize, 7usize);
        let mut x = vec![vec![0.0f32; w * h]];
        x[0][3 * w + 3] = 1.0;
        let mut hit = |dil: usize, dx: i32, dy: i32| {
            c.dil = dil;
            let o = c.forward(&x, w, h);
            o[0][((3 + dy) as usize) * w + (3 + dx) as usize]
        };
        assert_eq!(hit(1, 1, 0), 1.0, "dil=1 时 ±1 的抽头应命中");
        assert_eq!(hit(2, 1, 0), 0.0, "dil=2 时 ±1 间距上不应有抽头");
        assert_eq!(hit(2, 2, 0), 1.0, "dil=2 时 ±2 的抽头应命中");
        assert_eq!(hit(2, 2, 2), 1.0, "对角线 ±2 也应命中");
    }

    #[test]
    fn eps_denoiser_maps_noise_to_x0() {
        use crate::diffuse::Denoiser;
        // 网络输出恒 0 的 ε ⇒ x0 应等于 x_t/√ᾱ
        let (w, h) = (4usize, 4usize);
        let mut net = Net::new(2, &[], 3);
        net.linear_out = true;
        for l in net.layers.iter_mut() {
            l.b.fill(0.0);
            l.w.fill(0.0);
            l.scale.fill(0.0);
        }
        let abar = vec![1.0f32, 0.5, 0.0];
        let d = EpsDen {
            net,
            abar: abar.clone(),
            eps_scale: 1.0,
        };
        let xt = vec![0.4f32; w * h];
        let x0 = d.predict_x0(&xt, 1.0, None, w, h); // t_frac=1 ⇒ ᾱ=0 ⇒ 走 clamp
        assert_eq!(x0.len(), w * h);
        let x0b = d.predict_x0(&xt, 0.5, None, w, h); // ᾱ=0.5 ⇒ 0.4/√0.5≈0.566
        assert!(
            (x0b[0] - 0.4 / 0.5f32.sqrt()).abs() < 1e-4,
            "实际 {}",
            x0b[0]
        );
    }

    #[test]
    fn highpass_term_penalizes_missing_detail() {
        let (w, h) = (8usize, 8usize);
        let mut tgt = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                tgt[y * w + x] = if (x + y) % 2 == 0 { 1.0 } else { -1.0 };
            }
        }
        let maps = vec![vec![0.0f32; w * h]];
        let mk = |hp: f32| -> f32 {
            let mut n = Net::new(1, &[], 3);
            n.linear_out = true;
            n.highpass = hp;
            for l in n.layers.iter_mut() {
                l.w.fill(0.0);
                l.b.fill(0.0);
                l.scale.fill(1.0);
            }
            n.train_batch_x(&[(maps.clone(), tgt.clone())], w, h, 0.0, false)
        };
        let a = mk(0.0);
        let b = mk(1.0);
        assert!(b > a * 1.5, "高通项应显著加大损失：{a:.3} -> {b:.3}");
    }

    #[test]
    fn v_denoiser_maps_v_to_x0() {
        use crate::diffuse::Denoiser;
        // 网络输出恒 0 的 v ⇒ x̂0 = √ᾱ·x_t
        let (w, h) = (4usize, 4usize);
        let mut net = Net::new(2, &[], 3);
        net.linear_out = true;
        for l in net.layers.iter_mut() {
            l.b.fill(0.0);
            l.w.fill(0.0);
            l.scale.fill(0.0);
        }
        let abar = vec![1.0f32, 0.25, 0.0];
        let d = VDen {
            net,
            abar: abar.clone(),
            extra: Vec::new(),
            clamp: true,
            class: None,
        };
        let xt = vec![0.8f32; w * h];
        let x0 = d.predict_x0(&xt, 0.5, None, w, h); // ᾱ=0.25 ⇒ 0.5*0.8=0.4
        assert!((x0[0] - 0.4).abs() < 1e-4, "实际 {}", x0[0]);
        let x0b = d.predict_x0(&xt, 0.0, None, w, h); // ᾱ=1 ⇒ x_t
        assert!((x0b[0] - 0.8).abs() < 1e-4, "实际 {}", x0b[0]);
    }

    #[test]
    fn film_modulates_by_class() {
        let (w, h) = (6usize, 6usize);
        let mut net = Net::new(1, &[4], 9);
        net.set_film(2);
        // 手工把两类的 γ 设成不同值
        {
            let f = net.films[0].as_mut().unwrap();
            for v in f.g[0..4].iter_mut() {
                *v = 2.0;
            }
            for v in f.g[4..8].iter_mut() {
                *v = 0.5;
            }
        }
        let x = vec![vec![0.7f32; w * h]];
        let a = net.forward_xc(&x, Some(0), w, h);
        let b = net.forward_xc(&x, Some(1), w, h);
        let c = net.forward_xc(&x, None, w, h); // 空类 γ=1
        assert!((a[0] - b[0]).abs() > 1e-4, "不同类别应给出不同输出");
        assert!((c[0] - a[0]).abs() > 1e-4, "空类应与类 0 不同");
    }

    #[test]
    fn film_learns_class_offsets() {
        let (w, h) = (6usize, 6usize);
        let mut net = Net::new(1, &[4], 11);
        net.set_film(2);
        net.linear_out = true;
        let maps = vec![vec![0.5f32; w * h]];
        // 类 0 目标 0.2、类 1 目标 0.8
        let t0 = vec![0.2f32; w * h];
        let t1 = vec![0.8f32; w * h];
        let batch = vec![
            (maps.clone(), t0.clone(), Some(0usize)),
            (maps.clone(), t1.clone(), Some(1usize)),
        ];
        for _ in 0..300 {
            net.train_batch_xc(&batch, w, h, 0.05, true);
        }
        let a = net.forward_xc(&maps, Some(0), w, h)[0];
        let b = net.forward_xc(&maps, Some(1), w, h)[0];
        assert!(b - a > 0.2, "两类应被区分：{a:.3} vs {b:.3}");
    }

    #[test]
    fn coh_loss_grad_matches_finite_difference() {
        let (w, h) = (9usize, 8usize);
        let mut s = 11u64;
        let mut rnd = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 11) as f32 / (1u64 << 53) as f32 - 0.5
        };
        let x0: Vec<f32> = (0..w * h).map(|_| rnd()).collect();
        let (cstar, tstar, lam, wt) = (0.573f32, 0.0333f32, 1.0f32, 0.1f32);
        let (_, grad) = super::coh_loss_grad(&x0, w, h, cstar, tstar, lam, wt);
        let eps = 1e-3f32;
        for &i in &[w + 2, 3 * w + 4, 5 * w + 5] {
            let mut up = x0.clone();
            up[i] += eps;
            let mut dn = x0.clone();
            dn[i] -= eps;
            let fu = super::coh_loss_grad(&up, w, h, cstar, tstar, lam, wt).0;
            let fd = super::coh_loss_grad(&dn, w, h, cstar, tstar, lam, wt).0;
            let num = (fu - fd) / (2.0 * eps);
            let ana = grad[i];
            assert!(
                (num - ana).abs() < 0.05 * num.abs().max(0.05),
                "像素 {i}: 数值 {num:.4} vs 解析 {ana:.4}"
            );
        }
    }

    #[test]
    fn sobel_adjoint_is_transpose() {
        let (w, h) = (7usize, 6usize);
        let mut s = 7u64;
        let mut rnd = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 11) as f32 / (1u64 << 53) as f32 - 0.5
        };
        let x: Vec<f32> = (0..w * h).map(|_| rnd()).collect();
        let cx: Vec<f32> = (0..w * h).map(|_| rnd()).collect();
        let cy: Vec<f32> = (0..w * h).map(|_| rnd()).collect();
        let (gx, gy) = super::sobel(&x, w, h);
        let lhs: f32 = (0..w * h).map(|i| gx[i] * cx[i] + gy[i] * cy[i]).sum();
        let adj = super::sobel_adjoint(&cx, &cy, w, h);
        let rhs: f32 = (0..w * h).map(|i| x[i] * adj[i]).sum();
        assert!(
            (lhs - rhs).abs() < 1e-3 * lhs.abs().max(1.0),
            "adjoint 不是转置：{lhs} vs {rhs}"
        );
    }

    #[test]
    fn denoiser_api_feeds_t_channel() {
        use crate::diffuse::Denoiser;
        let (w, h) = (4usize, 4usize);
        let mut net = Net::new(2, &[4], 5);
        net.set_bn(false);
        let xt = vec![0.3f32; w * h];
        let p = net.predict_x0(&xt, 0.5, None, w, h);
        assert_eq!(p.len(), w * h);
        assert!(p.iter().all(|v| (0.0..=1.0).contains(v)));
        assert_eq!(Denoiser::input_dim(&net), 2);
    }

    #[test]
    fn bn_backward_matches_numeric() {
        // 纯 BN：y = γ·x̂+β，loss = Σ dout·y，检查 dx 与有限差分一致
        let n = 6usize;
        let x0 = vec![0.2f32, 0.9, 0.4, 0.7, 0.1, 0.5];
        let dout = vec![0.3f32, -0.2, 0.5, 0.1, -0.4, 0.2];
        let loss = |x: &[f32]| -> f64 {
            let mut bn = Bn::new(1);
            bn.gamma[0] = 1.3;
            bn.beta[0] = -0.2;
            let (y, _) = bn.forward(&[x.to_vec()], n, true);
            y[0].iter().zip(&dout).map(|(a, b)| f64::from(a * b)).sum()
        };
        let mut bn = Bn::new(1);
        bn.gamma[0] = 1.3;
        bn.beta[0] = -0.2;
        let (_, xh) = bn.forward(std::slice::from_ref(&x0), n, true);
        let (dx, _dg, _db) = bn.backward(&xh, std::slice::from_ref(&dout), n);
        let eps = 1e-3f32;
        for i in 0..n {
            let mut up = x0.clone();
            up[i] += eps;
            let mut dn = x0.clone();
            dn[i] -= eps;
            let num = (loss(&up) - loss(&dn)) / (2.0 * f64::from(eps));
            let ana = f64::from(dx[0][i]);
            assert!(
                (num - ana).abs() < 0.05 * ana.abs().max(1e-2),
                "i={i}: 数值 {num:.4} vs 解析 {ana:.4}"
            );
        }
    }

    #[test]
    fn gradient_matches_finite_difference() {
        // 小网络：1→1 单层 + sigmoid，检查 dL/dw 与数值差分一致
        let w = 4;
        let h = 4;
        let img: Vec<f32> = (0..16).map(|i| ((i * 37) % 11) as f32 / 11.0).collect();
        let target: Vec<f32> = (0..16).map(|i| ((i * 53) % 13) as f32 / 13.0).collect();
        let mut net = Net::new(1, &[], 7);
        net.layers[0].ternary = false;
        let loss_at = |net: &Net| -> f64 {
            let p = net.forward(&img, None, w, h);
            let mut l = 0.0f64;
            for i in 0..w * h {
                let q = p[i].clamp(1e-6, 1.0 - 1e-6);
                l -= f64::from(target[i] * q.ln() + (1.0 - target[i]) * (1.0 - q).ln());
            }
            l
        };
        // 用一次 train_step 的梯度方向估计：解析梯度 = -(w_new - w_old) * n / lr
        let before = net.layers[0].w.clone();
        let lr = 1e-2;
        net.train_step(&img, &target, None, w, h, lr);
        let after = net.layers[0].w.clone();
        let mut checked = 0;
        for k in 0..before.len() {
            let ana = -((after[k] - before[k]) / lr) as f64 * (w * h) as f64; // 更新用的是均值损失
            if ana.abs() < 1e-3 {
                continue;
            }
            let eps = 1e-2f32;
            let mut up = Net::new(1, &[], 7);
            up.layers[0].ternary = false;
            up.layers[0].w = before.clone();
            up.layers[0].w[k] += eps;
            let mut dn = Net::new(1, &[], 7);
            dn.layers[0].ternary = false;
            dn.layers[0].w = before.clone();
            dn.layers[0].w[k] -= eps;
            let num = (loss_at(&up) - loss_at(&dn)) / (2.0 * f64::from(eps));
            assert!(
                (num - ana).abs() < 0.1 * ana.abs().max(1e-2),
                "k={k}: 数值 {num:.4} vs 解析 {ana:.4}"
            );
            checked += 1;
        }
        assert!(checked > 3, "应至少校验 3 个权重");
    }
}
