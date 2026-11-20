//! 训练：单步 / 批量梯度累积与参数更新（从 net.rs 下沉，impl 拆两块）。

use super::super::conv::{Sample, coh_loss_grad, sobel, sobel_adjoint};
use super::{FilmState, Net};

impl Net {
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
