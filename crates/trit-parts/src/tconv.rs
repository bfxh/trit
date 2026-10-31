//! 小型三值卷积栈（CPU）：3×3、same padding、隐藏层 ReLU、输出 sigmoid + BCE。
//!
//! 用途：证明「扩感受野要靠加深 + 共享权重」而不是把 patch 摊平进 MLP。
//! 权重三值（{-1,0,+1}，STE 直通），bias 与输出层保持 f32（与 `occ::Mlp` 同一取舍）。
//! 通道输入：第 0 通道是图像，后面可拼 one-hot 条件（常数图）。

/// 一层 3×3 卷积。
#[derive(Clone)]
pub struct Conv {
    pub cin: usize,
    pub cout: usize,
    /// `cout·cin·9`，布局 `((co*cin)+ci)*9 + (dy*3+dx)`。
    pub w: Vec<f32>,
    pub b: Vec<f32>,
    /// true = 前向用三值权重（STE 反传）；false = 原生 f32（用于梯度校验）。
    pub ternary: bool,
}

impl Conv {
    fn eff(&self, v: f32) -> f32 {
        if self.ternary { q(v) } else { v }
    }
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
            cin,
            cout,
            // 初始要越过三值死区（|w|<0.5 → 量化成 0），否则整层恒零
            w: (0..cout * cin * 9).map(|_| next() * 3.0).collect(),
            b: vec![0.0; cout],
            ternary: true,
        }
    }

    fn idx(&self, co: usize, ci: usize, k: usize) -> usize {
        (co * self.cin + ci) * 9 + k
    }

    #[allow(clippy::needless_range_loop)] // 三重坐标循环，索引写法更直白
    pub fn forward(&self, x: &[Vec<f32>], w: usize, h: usize) -> Vec<Vec<f32>> {
        let mut out = vec![vec![0.0f32; w * h]; self.cout];
        for co in 0..self.cout {
            for y in 0..h {
                for xx in 0..w {
                    let mut s = self.b[co];
                    for ci in 0..self.cin {
                        for dy in 0..3 {
                            let yy = y as i32 + dy as i32 - 1;
                            if yy < 0 || yy >= h as i32 {
                                continue;
                            }
                            for dx in 0..3 {
                                let xr = xx as i32 + dx as i32 - 1;
                                if xr < 0 || xr >= w as i32 {
                                    continue;
                                }
                                s += self.eff(self.w[self.idx(co, ci, dy * 3 + dx)])
                                    * x[ci][yy as usize * w + xr as usize];
                            }
                        }
                    }
                    out[co][y * w + xx] = s;
                }
            }
        }
        out
    }

    /// 反向：给定 `dL/dout`，返回 `dL/dx`，并把 `dL/dw`、`dL/db` 用 SGD 直接更新。
    #[allow(clippy::needless_range_loop)] // 同上
    fn backward(
        &mut self,
        x: &[Vec<f32>],
        w: usize,
        h: usize,
        dout: &[Vec<f32>],
        lr: f32,
    ) -> Vec<Vec<f32>> {
        let mut dx = vec![vec![0.0f32; w * h]; self.cin];
        let mut dw = vec![0.0f32; self.w.len()];
        let mut db = vec![0.0f32; self.cout];
        for co in 0..self.cout {
            for y in 0..h {
                for xx in 0..w {
                    let g = dout[co][y * w + xx];
                    if g == 0.0 {
                        continue;
                    }
                    db[co] += g;
                    for ci in 0..self.cin {
                        for dy in 0..3 {
                            let yy = y as i32 + dy as i32 - 1;
                            if yy < 0 || yy >= h as i32 {
                                continue;
                            }
                            for dxk in 0..3 {
                                let xr = xx as i32 + dxk as i32 - 1;
                                if xr < 0 || xr >= w as i32 {
                                    continue;
                                }
                                let k = dy * 3 + dxk;
                                let xi = yy as usize * w + xr as usize;
                                let xv = x[ci][xi];
                                // STE：权重梯度按量化后的输入/输出算，梯度直通到实值 w
                                dw[self.idx(co, ci, k)] += g * xv;
                                dx[ci][xi] += g * self.eff(self.w[self.idx(co, ci, k)]);
                            }
                        }
                    }
                }
            }
        }
        // `dout` 已经含 1/n，这里不再除
        for (wi, gi) in self.w.iter_mut().zip(&dw) {
            *wi -= lr * gi;
        }
        for (bi, gi) in self.b.iter_mut().zip(&db) {
            *bi -= lr * gi;
        }
        dx
    }
}

/// 全卷积栈：隐藏层 ReLU，最后一层 sigmoid。
pub struct Net {
    pub layers: Vec<Conv>,
    /// 条件通道数（追加在输入后面，常数图）。
    pub cond: usize,
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
        Net { layers, cond: 0 }
    }

    /// 把条件拼成常数通道。
    fn input(&self, img: &[f32], cond: Option<usize>, w: usize, h: usize) -> Vec<Vec<f32>> {
        let mut x = vec![img.to_vec()];
        for k in 0..self.cond {
            x.push(vec![(cond == Some(k)) as u8 as f32; w * h]);
        }
        x
    }

    /// 前向：返回 (输出图, 每层激活（含 ReLU 后）)。
    pub fn forward(&self, img: &[f32], cond: Option<usize>, w: usize, h: usize) -> Vec<f32> {
        let mut x = self.input(img, cond, w, h);
        for (i, l) in self.layers.iter().enumerate() {
            let mut y = l.forward(&x, w, h);
            if i + 1 < self.layers.len() {
                for ch in y.iter_mut() {
                    for v in ch.iter_mut() {
                        *v = v.max(0.0);
                    }
                }
            } else {
                for ch in y.iter_mut() {
                    for v in ch.iter_mut() {
                        *v = 1.0 / (1.0 + (-*v).exp());
                    }
                }
            }
            x = y;
        }
        x.pop().unwrap()
    }

    /// 单张图 BCE 训练一步；返回 loss。
    pub fn train_step(
        &mut self,
        img: &[f32],
        target: &[f32],
        cond: Option<usize>,
        w: usize,
        h: usize,
        lr: f32,
    ) -> f32 {
        let mut acts: Vec<Vec<Vec<f32>>> = Vec::new();
        let mut x = self.input(img, cond, w, h);
        for (i, l) in self.layers.iter().enumerate() {
            let mut y = l.forward(&x, w, h);
            if i + 1 < self.layers.len() {
                for ch in y.iter_mut() {
                    for v in ch.iter_mut() {
                        *v = v.max(0.0);
                    }
                }
            } else {
                for ch in y.iter_mut() {
                    for v in ch.iter_mut() {
                        *v = 1.0 / (1.0 + (-*v).exp());
                    }
                }
            }
            acts.push(x.clone());
            x = y;
        }
        let pred = &x[0];
        let mut loss = 0.0f64;
        let mut d = vec![vec![0.0f32; w * h]; 1];
        for i in 0..w * h {
            let p = pred[i].clamp(1e-6, 1.0 - 1e-6);
            let t = target[i];
            loss -= f64::from(t * p.ln() + (1.0 - t) * (1.0 - p).ln());
            d[0][i] = (p - t) / (w * h) as f32;
        }
        // 反向
        for li in (0..self.layers.len()).rev() {
            let mut dx = self.layers[li].backward(&acts[li], w, h, &d, lr);
            if li > 0 {
                // 穿过 ReLU：mask = 该层输入（= 上一层 ReLU 输出）> 0
                for c in 0..dx.len() {
                    for i in 0..w * h {
                        if acts[li][c][i] <= 0.0 {
                            dx[c][i] = 0.0;
                        }
                    }
                }
            }
            d = dx;
        }
        loss as f32
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
