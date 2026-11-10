use super::bn::{AuxCls, Bn, Film};
use super::conv::{Conv, Sample, coh_loss_grad, sobel, sobel_adjoint};

/// FiLM 反向所需：(FiLM 前的 conv 输出, 该类 γ, 类别)。
type FilmState = (Vec<Vec<f32>>, Vec<f32>, Option<usize>);

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
