//! 反向传播的 CPU 参考实现（f64），只做本网络用到的四种算子。
//!
//! 为什么先有 CPU 版：**有限差分只能验证「直通前向」的梯度**（三值量化对 `w` 的导数几乎处处为 0），
//! 所以判据拆成两层：
//! 1. 对**有效权重** `E`（`E = q·scale`）做解析反向，用中心差分逐元素对拍 `∂L/∂x / ∂L/∂E / ∂L/∂b`；
//! 2. `E → latent` 的映射按 `trit-learn` 的口径（[`trit_learn::Ste`]）单独验证：
//!    `grad_latent = Ste::backward(w, s, s·dE)`、`grad_scale = Σ dE·q`。
//!
//! GPU 内核只负责第 1 层（`dE / dx / db`），第 2 层在主机侧做——权重数量远小于激活，没必要上卡。

use trit_learn::Ste;

/// 一个 3×3 / 1×1 卷积的形状（`pad = kk/2`，NHWC，N=1）。
#[derive(Clone, Copy, Debug)]
pub struct ConvSpec {
    pub h: usize,
    pub w: usize,
    pub cin: usize,
    pub cout: usize,
    pub kk: usize,
    pub relu: bool,
}

/// 卷积前向：`y[oy,ox,oc] = Σ x[iy,ix,ic]·E[(ky,kk+kx),ic,oc] + b[oc]`。
pub fn conv_f64(x: &[f64], e: &[f64], b: &[f64], s: &ConvSpec) -> Vec<f64> {
    let pad = s.kk / 2;
    let mut y = vec![0.0; s.h * s.w * s.cout];
    for oy in 0..s.h {
        for ox in 0..s.w {
            for oc in 0..s.cout {
                let mut acc = b[oc];
                for ky in 0..s.kk {
                    let iy = oy as isize + ky as isize - pad as isize;
                    if iy < 0 || iy >= s.h as isize {
                        continue;
                    }
                    for kx in 0..s.kk {
                        let ix = ox as isize + kx as isize - pad as isize;
                        if ix < 0 || ix >= s.w as isize {
                            continue;
                        }
                        let xb = ((iy as usize) * s.w + ix as usize) * s.cin;
                        let wb = ((ky * s.kk + kx) * s.cin) * s.cout;
                        for ic in 0..s.cin {
                            acc += x[xb + ic] * e[wb + ic * s.cout + oc];
                        }
                    }
                }
                y[(oy * s.w + ox) * s.cout + oc] = if s.relu { acc.max(0.0) } else { acc };
            }
        }
    }
    y
}

/// 反向的产物。`dw` 是**对有效权重**的梯度（STE 之前）。
pub struct ConvGrad {
    pub dx: Vec<f64>,
    pub dw: Vec<f64>,
    pub db: Vec<f64>,
}

/// 卷积反向：`dy` 是 `∂L/∂y`（ReLU 之后）。
pub fn conv_backward_f64(x: &[f64], y: &[f64], dy: &[f64], e: &[f64], s: &ConvSpec) -> ConvGrad {
    let pad = s.kk / 2;
    let mut dx = vec![0.0; s.h * s.w * s.cin];
    let mut dw = vec![0.0; s.kk * s.kk * s.cin * s.cout];
    let mut db = vec![0.0; s.cout];
    for oy in 0..s.h {
        for ox in 0..s.w {
            for (oc, db_slot) in db.iter_mut().enumerate() {
                let idx = (oy * s.w + ox) * s.cout + oc;
                // ReLU 的门：前向输出 ≤ 0 的位置不回传
                let g = if s.relu && y[idx] <= 0.0 {
                    0.0
                } else {
                    dy[idx]
                };
                if g == 0.0 {
                    continue;
                }
                *db_slot += g;
                for ky in 0..s.kk {
                    let iy = oy as isize + ky as isize - pad as isize;
                    if iy < 0 || iy >= s.h as isize {
                        continue;
                    }
                    for kx in 0..s.kk {
                        let ix = ox as isize + kx as isize - pad as isize;
                        if ix < 0 || ix >= s.w as isize {
                            continue;
                        }
                        let xb = ((iy as usize) * s.w + ix as usize) * s.cin;
                        let wb = ((ky * s.kk + kx) * s.cin) * s.cout;
                        for ic in 0..s.cin {
                            let wi = wb + ic * s.cout + oc;
                            dw[wi] += x[xb + ic] * g;
                            dx[xb + ic] += e[wi] * g;
                        }
                    }
                }
            }
        }
    }
    ConvGrad { dx, dw, db }
}

/// 2×2 stride-2 最大池化（NHWC）。
pub fn pool_f64(x: &[f64], h: usize, w: usize, c: usize) -> Vec<f64> {
    let (oh, ow) = (h / 2, w / 2);
    let mut y = vec![0.0; oh * ow * c];
    for oy in 0..oh {
        for ox in 0..ow {
            for ch in 0..c {
                let mut m = f64::NEG_INFINITY;
                for dy in 0..2 {
                    for dx in 0..2 {
                        m = m.max(x[((oy * 2 + dy) * w + ox * 2 + dx) * c + ch]);
                    }
                }
                y[(oy * ow + ox) * c + ch] = m;
            }
        }
    }
    y
}

/// 池化反向：梯度只回给前向取到最大值的位置（并列时取 **dy 优先、再 dx** 的第一个，与前向同序）。
pub fn pool_backward_f64(x: &[f64], dy: &[f64], h: usize, w: usize, c: usize) -> Vec<f64> {
    let (oh, ow) = (h / 2, w / 2);
    let mut dx = vec![0.0; h * w * c];
    for oy in 0..oh {
        for ox in 0..ow {
            for ch in 0..c {
                let g = dy[(oy * ow + ox) * c + ch];
                let mut best = f64::NEG_INFINITY;
                let mut at = (oy * 2, ox * 2);
                for sub_y in 0..2 {
                    for sub_x in 0..2 {
                        let v = x[((oy * 2 + sub_y) * w + ox * 2 + sub_x) * c + ch];
                        if v > best {
                            best = v;
                            at = (oy * 2 + sub_y, ox * 2 + sub_x);
                        }
                    }
                }
                dx[(at.0 * w + at.1) * c + ch] += g;
            }
        }
    }
    dx
}

/// 通道拼接（NHWC）：`y = [a | b]`。
pub fn cat_f64(a: &[f64], b: &[f64], hw: usize, ca: usize, cb: usize) -> Vec<f64> {
    let mut y = vec![0.0; hw * (ca + cb)];
    for p in 0..hw {
        y[p * (ca + cb)..p * (ca + cb) + ca].copy_from_slice(&a[p * ca..(p + 1) * ca]);
        y[p * (ca + cb) + ca..(p + 1) * (ca + cb)].copy_from_slice(&b[p * cb..(p + 1) * cb]);
    }
    y
}

/// 拼接反向：按通道轴切回去。
pub fn cat_backward_f64(dy: &[f64], hw: usize, ca: usize, cb: usize) -> (Vec<f64>, Vec<f64>) {
    let mut da = vec![0.0; hw * ca];
    let mut db = vec![0.0; hw * cb];
    for p in 0..hw {
        da[p * ca..(p + 1) * ca].copy_from_slice(&dy[p * (ca + cb)..p * (ca + cb) + ca]);
        db[p * cb..(p + 1) * cb].copy_from_slice(&dy[p * (ca + cb) + ca..(p + 1) * (ca + cb)]);
    }
    (da, db)
}

/// 转置卷积 `k=2, s=2` 前向（无 ReLU）：
/// `y[2iy+kh, 2ix+kw, co] = Σ_ci x[iy,ix,ci]·E[(kh*2+kw),ci,co] + b[co]`。
pub fn convt_f64(
    x: &[f64],
    e: &[f64],
    b: &[f64],
    h: usize,
    w: usize,
    cin: usize,
    cout: usize,
) -> Vec<f64> {
    let mut y = vec![0.0; (h * 2) * (w * 2) * cout];
    for iy in 0..h {
        for ix in 0..w {
            for co in 0..cout {
                for kh in 0..2 {
                    for kw in 0..2 {
                        let mut acc = b[co];
                        for ci in 0..cin {
                            let wi = ((kh * 2 + kw) * cin + ci) * cout + co;
                            acc += x[(iy * w + ix) * cin + ci] * e[wi];
                        }
                        y[((iy * 2 + kh) * (w * 2) + ix * 2 + kw) * cout + co] = acc;
                    }
                }
            }
        }
    }
    y
}

/// 转置卷积反向（无 ReLU）。
pub fn convt_backward_f64(
    x: &[f64],
    dy: &[f64],
    e: &[f64],
    h: usize,
    w: usize,
    cin: usize,
    cout: usize,
) -> ConvGrad {
    let mut dx = vec![0.0; h * w * cin];
    let mut dw = vec![0.0; 4 * cin * cout];
    let mut db = vec![0.0; cout];
    for iy in 0..h {
        for ix in 0..w {
            for (co, db_slot) in db.iter_mut().enumerate() {
                for kh in 0..2 {
                    for kw in 0..2 {
                        let g = dy[((iy * 2 + kh) * (w * 2) + ix * 2 + kw) * cout + co];
                        *db_slot += g;
                        for ci in 0..cin {
                            let wi = ((kh * 2 + kw) * cin + ci) * cout + co;
                            dw[wi] += x[(iy * w + ix) * cin + ci] * g;
                            dx[(iy * w + ix) * cin + ci] += e[wi] * g;
                        }
                    }
                }
            }
        }
    }
    ConvGrad { dx, dw, db }
}

/// 把「对有效权重 `E = q·scale`」的梯度映射回 latent 与每通道 scale。
///
/// 口径与 `trit-train` 的 `TernaryLinear::train_step` 一致：
/// `grad_q = dE·scale` → `grad_latent = Ste::backward(latent, scale, grad_q)`；
/// `grad_scale[oc] = Σ dE·q`（q 对 scale 视为常数）。
pub fn ste_apply(
    latent: &[f32],
    q: &[i32],
    scale: &[f32],
    d_e: &[f32],
    ste: Ste,
    cout: usize,
) -> (Vec<f32>, Vec<f32>) {
    assert_eq!(latent.len(), d_e.len());
    assert_eq!(q.len(), d_e.len());
    let mut d_latent = vec![0.0f32; d_e.len()];
    let mut d_scale = vec![0.0f32; scale.len()];
    for (wi, (&de, &qi)) in d_e.iter().zip(q).enumerate() {
        let oc = wi % cout;
        let s = scale[oc];
        d_latent[wi] = ste.backward(latent[wi], s, de * s);
        d_scale[oc] += de * qi as f32;
    }
    (d_latent, d_scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rng(seed: &mut u64) -> f64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        ((*seed % 2001) as f64 - 1000.0) / 1000.0
    }

    fn fill(seed: &mut u64, n: usize) -> Vec<f64> {
        (0..n).map(|_| rng(seed)).collect()
    }

    /// `L = 0.5·Σ(y − t)²`，返回 (L, dL/dy)。
    fn loss_and_dy(y: &[f64], t: &[f64]) -> (f64, Vec<f64>) {
        let mut loss = 0.0;
        let mut dy = vec![0.0; y.len()];
        for ((yi, ti), d) in y.iter().zip(t).zip(dy.iter_mut()) {
            let diff = yi - ti;
            loss += 0.5 * diff * diff;
            *d = diff;
        }
        (loss, dy)
    }

    fn rel_err(a: f64, b: f64) -> f64 {
        (a - b).abs() / a.abs().max(b.abs()).max(1e-6)
    }

    #[test]
    fn conv_grad_matches_finite_difference() {
        for relu in [false, true] {
            for kk in [1usize, 3] {
                let s = ConvSpec {
                    h: 4,
                    w: 5,
                    cin: 2,
                    cout: 3,
                    kk,
                    relu,
                };
                let mut seed = 0x1234_5678_9abc_def1u64;
                let x = fill(&mut seed, s.h * s.w * s.cin);
                let e = fill(&mut seed, s.kk * s.kk * s.cin * s.cout);
                let b = fill(&mut seed, s.cout);
                let t = fill(&mut seed, s.h * s.w * s.cout);

                let y = conv_f64(&x, &e, &b, &s);
                let (_, dy) = loss_and_dy(&y, &t);
                let g = conv_backward_f64(&x, &y, &dy, &e, &s);

                let h = 1e-6;
                let mut xm = x.clone();
                for i in 0..g.dx.len() {
                    let old = xm[i];
                    xm[i] = old + h;
                    let up = loss_and_dy(&conv_f64(&xm, &e, &b, &s), &t).0;
                    xm[i] = old - h;
                    let dn = loss_and_dy(&conv_f64(&xm, &e, &b, &s), &t).0;
                    xm[i] = old;
                    let got = (up - dn) / (2.0 * h);
                    assert!(
                        rel_err(got, g.dx[i]) < 1e-6,
                        "dx[{i}] kk={kk} relu={relu}: {got} vs {}",
                        g.dx[i]
                    );
                }
                let mut em = e.clone();
                for i in 0..g.dw.len() {
                    let old = em[i];
                    em[i] = old + h;
                    let up = loss_and_dy(&conv_f64(&x, &em, &b, &s), &t).0;
                    em[i] = old - h;
                    let dn = loss_and_dy(&conv_f64(&x, &em, &b, &s), &t).0;
                    em[i] = old;
                    let got = (up - dn) / (2.0 * h);
                    assert!(
                        rel_err(got, g.dw[i]) < 1e-6,
                        "dE[{i}] kk={kk} relu={relu}: {got} vs {}",
                        g.dw[i]
                    );
                }
                let mut bm = b.clone();
                for i in 0..g.db.len() {
                    let old = bm[i];
                    bm[i] = old + h;
                    let up = loss_and_dy(&conv_f64(&x, &e, &bm, &s), &t).0;
                    bm[i] = old - h;
                    let dn = loss_and_dy(&conv_f64(&x, &e, &bm, &s), &t).0;
                    bm[i] = old;
                    let got = (up - dn) / (2.0 * h);
                    assert!(
                        rel_err(got, g.db[i]) < 1e-6,
                        "db[{i}] kk={kk} relu={relu}: {got} vs {}",
                        g.db[i]
                    );
                }
            }
        }
    }

    #[test]
    fn convt_grad_matches_finite_difference() {
        let (h, w, cin, cout) = (3usize, 4usize, 2usize, 3usize);
        let mut seed = 0x0bad_f00d_dead_beefu64;
        let x = fill(&mut seed, h * w * cin);
        let e = fill(&mut seed, 4 * cin * cout);
        let b = fill(&mut seed, cout);
        let t = fill(&mut seed, (h * 2) * (w * 2) * cout);

        let y = convt_f64(&x, &e, &b, h, w, cin, cout);
        let (_, dy) = loss_and_dy(&y, &t);
        let g = convt_backward_f64(&x, &dy, &e, h, w, cin, cout);

        let step = 1e-6;
        let mut xm = x.clone();
        for i in 0..g.dx.len() {
            let old = xm[i];
            xm[i] = old + step;
            let up = loss_and_dy(&convt_f64(&xm, &e, &b, h, w, cin, cout), &t).0;
            xm[i] = old - step;
            let dn = loss_and_dy(&convt_f64(&xm, &e, &b, h, w, cin, cout), &t).0;
            xm[i] = old;
            assert!(
                rel_err((up - dn) / (2.0 * step), g.dx[i]) < 1e-6,
                "convt dx[{i}]"
            );
        }
        let mut em = e.clone();
        for i in 0..g.dw.len() {
            let old = em[i];
            em[i] = old + step;
            let up = loss_and_dy(&convt_f64(&x, &em, &b, h, w, cin, cout), &t).0;
            em[i] = old - step;
            let dn = loss_and_dy(&convt_f64(&x, &em, &b, h, w, cin, cout), &t).0;
            em[i] = old;
            assert!(
                rel_err((up - dn) / (2.0 * step), g.dw[i]) < 1e-6,
                "convt dE[{i}]"
            );
        }
        let mut bm = b.clone();
        for i in 0..g.db.len() {
            let old = bm[i];
            bm[i] = old + step;
            let up = loss_and_dy(&convt_f64(&x, &e, &bm, h, w, cin, cout), &t).0;
            bm[i] = old - step;
            let dn = loss_and_dy(&convt_f64(&x, &e, &bm, h, w, cin, cout), &t).0;
            bm[i] = old;
            assert!(
                rel_err((up - dn) / (2.0 * step), g.db[i]) < 1e-6,
                "convt db[{i}]"
            );
        }
    }

    #[test]
    fn pool_backward_routes_to_argmax() {
        // 2×2 池化，c=2；第二个窗口里 dy 优先取到 (1,0) 的较大值
        let (h, w, c) = (2usize, 4usize, 2usize);
        let x: Vec<f64> = vec![
            1.0, -1.0, 3.0, 0.5, 2.0, 4.0, 0.0, 9.0, 5.0, 1.0, 6.0, 2.0, 0.0, -2.0, 0.5, 3.0,
        ];
        let y = pool_f64(&x, h, w, c);
        // 窗口 (0,0): c0 最大 6@(1,1)；c1 最大 2@(1,1)。窗口 (0,1): c0 最大 2@(0,2)；c1 最大 9@(0,3)
        assert_eq!(y, vec![6.0, 2.0, 2.0, 9.0]);
        let dy = vec![1.0; y.len()];
        let dx = pool_backward_f64(&x, &dy, h, w, c);
        assert_eq!(dx.iter().sum::<f64>(), 4.0);
        assert_eq!(dx[(w + 1) * c], 1.0);
        assert_eq!(dx[(w + 1) * c + 1], 1.0);
        assert_eq!(dx[2 * c], 1.0);
        assert_eq!(dx[3 * c + 1], 1.0);
    }

    #[test]
    fn ste_matches_trit_train_linear() {
        let (rows, cols) = (2usize, 3usize);
        let lr = 0.05f32;
        let scale0 = 0.5f32;
        let latent: Vec<f32> = vec![0.31, -0.72, 0.05, -0.44, 0.83, -0.12];
        let x: Vec<f32> = vec![0.6, -0.9, 0.25];
        let target: Vec<f32> = vec![0.4, -0.7];

        for ste in [Ste::Identity, Ste::Clipped { ratio: 1.0 }, Ste::Soft] {
            let mut layer = trit_train::layer::TernaryLinear::new(
                rows,
                cols,
                |r, c| latent[r * cols + c],
                ste,
                lr,
                scale0,
            );
            layer.train_step(&x, &target);

            // 我们的口径：E = q·scale；dE[c·rows+r] = dy_r·x_c（卷积布局：oc 在最低位）
            let y: Vec<f32> = (0..rows)
                .map(|r| {
                    let q =
                        trit_train::quantize_with_scale(&latent[r * cols..(r + 1) * cols], scale0);
                    scale0 * trit_train::layer::dot(&q, &x, false)
                })
                .collect();
            let dy: Vec<f32> = (0..rows).map(|r| y[r] - target[r]).collect();
            let mut d_e = vec![0.0f32; rows * cols];
            let mut q_cw = vec![0i32; rows * cols];
            let mut latent_cw = vec![0.0f32; rows * cols];
            for r in 0..rows {
                let q = trit_train::quantize_with_scale(&latent[r * cols..(r + 1) * cols], scale0);
                for c in 0..cols {
                    let wi = c * rows + r;
                    d_e[wi] = dy[r] * x[c];
                    q_cw[wi] = i32::from(q[c]);
                    latent_cw[wi] = latent[r * cols + c];
                }
            }
            let (d_latent, d_scale) =
                ste_apply(&latent_cw, &q_cw, &vec![scale0; rows], &d_e, ste, rows);

            for r in 0..rows {
                for c in 0..cols {
                    let want = latent[r * cols + c] - lr * d_latent[c * rows + r];
                    let got = layer.latent()[r * cols + c];
                    assert!(
                        (got - want).abs() < 1e-6,
                        "{ste:?} latent[{r}][{c}]: {got} vs {want}"
                    );
                }
            }
            for (r, &ds) in d_scale.iter().enumerate() {
                let want = (scale0 - lr * ds).max(1e-4);
                let got = layer.scales()[r];
                assert!(
                    (got - want).abs() < 1e-6,
                    "{ste:?} scale[{r}]: {got} vs {want}"
                );
            }
        }
    }

    #[test]
    fn cat_backward_splits_exactly() {
        let (hw, ca, cb) = (3usize, 2usize, 3usize);
        let a: Vec<f64> = (0..hw * ca).map(|i| i as f64).collect();
        let b: Vec<f64> = (0..hw * cb).map(|i| -(i as f64)).collect();
        let y = cat_f64(&a, &b, hw, ca, cb);
        let (da, db) = cat_backward_f64(&y, hw, ca, cb);
        assert_eq!(da, a);
        assert_eq!(db, b);
    }
}

/// per-channel affine 前向（参考实现）：`y = relu(γ_c·v + β_c)`。
pub fn affine_f64(v: &[f64], gamma: &[f64], beta: &[f64], cout: usize, relu: bool) -> Vec<f64> {
    v.iter()
        .enumerate()
        .map(|(i, &x)| {
            let t = gamma[i % cout] * x + beta[i % cout];
            if relu { t.max(0.0) } else { t }
        })
        .collect()
}

/// affine 反向：返回 `(dL/dv, dγ, dβ)`。
pub fn affine_backward_f64(
    v: &[f64],
    gamma: &[f64],
    beta: &[f64],
    dy: &[f64],
    cout: usize,
    relu: bool,
) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let mut dv = vec![0.0; v.len()];
    let mut dg = vec![0.0; cout];
    let mut db = vec![0.0; cout];
    for (i, &x) in v.iter().enumerate() {
        let oc = i % cout;
        let t = gamma[oc] * x + beta[oc];
        let m = if relu && t <= 0.0 { 0.0 } else { 1.0 };
        dv[i] = dy[i] * m * gamma[oc];
        dg[oc] += dy[i] * m * x;
        db[oc] += dy[i] * m;
    }
    (dv, dg, db)
}

#[cfg(test)]
mod affine_tests {
    use super::*;

    #[test]
    fn affine_grad_matches_finite_difference() {
        let (cout, hw) = (3usize, 5usize);
        let n = cout * hw;
        let mut seed = 0x1234_9876_abcd_0001u64;
        let mut fill = |k: usize| {
            (0..k)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    ((seed % 2001) as f64 - 1000.0) / 700.0
                })
                .collect::<Vec<f64>>()
        };
        let v = fill(n);
        let gamma = fill(cout);
        let beta = fill(cout);
        let dy = fill(n);
        let t = fill(n);
        let (dv, dg, db) = affine_backward_f64(&v, &gamma, &beta, &dy, cout, true);

        let loss = |v: &[f64], g: &[f64], b: &[f64]| -> f64 {
            affine_f64(v, g, b, cout, true)
                .iter()
                .zip(&t)
                .map(|(y, tt)| 0.5 * (y - tt) * (y - tt))
                .sum()
        };
        let h = 1e-6;
        // dL/dv = dy (MSE) 再乘 affine 的雅可比 —— 用链式关系验证：把 dy 设成 (y-t)
        let y = affine_f64(&v, &gamma, &beta, cout, true);
        let dy2: Vec<f64> = y.iter().zip(&t).map(|(a, b)| a - b).collect();
        let (dv2, dg2, db2) = affine_backward_f64(&v, &gamma, &beta, &dy2, cout, true);
        for i in 0..n {
            let mut up = v.clone();
            up[i] += h;
            let mut dn = v.clone();
            dn[i] -= h;
            let num = (loss(&up, &gamma, &beta) - loss(&dn, &gamma, &beta)) / (2.0 * h);
            assert!(
                (num - dv2[i]).abs() < 1e-5,
                "dv[{i}]: {} vs {}",
                dv2[i],
                num
            );
        }
        for oc in 0..cout {
            let mut up = gamma.clone();
            up[oc] += h;
            let mut dn = gamma.clone();
            dn[oc] -= h;
            let num = (loss(&v, &up, &beta) - loss(&v, &dn, &beta)) / (2.0 * h);
            assert!(
                (num - dg2[oc]).abs() < 1e-5,
                "dg[{oc}]: {} vs {}",
                dg2[oc],
                num
            );
            let mut up = beta.clone();
            up[oc] += h;
            let mut dn = beta.clone();
            dn[oc] -= h;
            let num = (loss(&v, &gamma, &up) - loss(&v, &gamma, &dn)) / (2.0 * h);
            assert!(
                (num - db2[oc]).abs() < 1e-5,
                "db[{oc}]: {} vs {}",
                db2[oc],
                num
            );
        }
        let _ = (dv, dg, db);
    }
}
