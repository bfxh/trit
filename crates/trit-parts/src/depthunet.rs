//! 多尺度（**降采样编码器**）深度头：CPU 参考实现。
//!
//! 结构（NHWC 之外按通道平面组织，与 [`crate::tconv::Conv`] 一致）：
//!
//! ```text
//! x 64² → c1(relu) → c2(relu) → avgpool2 → 32²
//!      → c3(relu) → c4(relu) → avgpool2 → 16²
//!      → c5(relu) → up2 → 32² → add(c4 输出)      ← 跳连
//!      → c6(relu) → up2 → 64²
//!      → c7(relu) → c8(线性) → out
//! ```
//!
//! 目的：把「感受野靠**降采样**长出来」这条杠杆也测掉（§6.91 的最后一档）。
//! 这是 CPU 参考实现——先回答「有没有增益」，有增益再上 GPU。

use crate::tconv::Conv;

/// 平均池化 2×2 / stride 2（逐平面）。
pub fn avgpool2(x: &[f32], h: usize, w: usize) -> (Vec<f32>, usize, usize) {
    let (oh, ow) = (h / 2, w / 2);
    let mut out = vec![0.0f32; oh * ow];
    for y in 0..oh {
        for xx in 0..ow {
            let mut s = 0.0;
            for dy in 0..2 {
                for dx in 0..2 {
                    s += x[(y * 2 + dy) * w + xx * 2 + dx];
                }
            }
            out[y * ow + xx] = s * 0.25;
        }
    }
    (out, oh, ow)
}

/// 池化反向：每个输入拿到 `g/4`。
pub fn avgpool2_bwd(g: &[f32], h: usize, w: usize) -> Vec<f32> {
    let (oh, ow) = (h / 2, w / 2);
    let mut out = vec![0.0f32; h * w];
    for y in 0..oh {
        for xx in 0..ow {
            let v = g[y * ow + xx] * 0.25;
            for dy in 0..2 {
                for dx in 0..2 {
                    out[(y * 2 + dy) * w + xx * 2 + dx] = v;
                }
            }
        }
    }
    out
}

/// 最近邻 2× 上采样（逐平面）。
pub fn up2(x: &[f32], h: usize, w: usize) -> (Vec<f32>, usize, usize) {
    let (uh, uw) = (h * 2, w * 2);
    let mut out = vec![0.0f32; uh * uw];
    for y in 0..h {
        for xx in 0..w {
            let v = x[y * w + xx];
            for dy in 0..2 {
                for dx in 0..2 {
                    out[(y * 2 + dy) * uw + xx * 2 + dx] = v;
                }
            }
        }
    }
    (out, uh, uw)
}

/// 上采样反向：把 2×2 的梯度求和回原分辨率。
pub fn up2_bwd(g: &[f32], h: usize, w: usize) -> Vec<f32> {
    let uw = w * 2;
    let mut out = vec![0.0f32; h * w];
    for y in 0..h {
        for xx in 0..w {
            let mut s = 0.0;
            for dy in 0..2 {
                for dx in 0..2 {
                    s += g[(y * 2 + dy) * uw + xx * 2 + dx];
                }
            }
            out[y * w + xx] = s;
        }
    }
    out
}

fn add_planes(a: &[Vec<f32>], b: &[Vec<f32>]) -> Vec<Vec<f32>> {
    a.iter()
        .zip(b)
        .map(|(x, y)| x.iter().zip(y).map(|(u, v)| u + v).collect())
        .collect()
}

fn relu(v: Vec<Vec<f32>>) -> Vec<Vec<f32>> {
    v.into_iter()
        .map(|m| m.into_iter().map(|x| x.max(0.0)).collect())
        .collect()
}

fn mask(d: &[Vec<f32>], pre: &[Vec<f32>]) -> Vec<Vec<f32>> {
    d.iter()
        .zip(pre)
        .map(|(g, o)| {
            g.iter()
                .zip(o)
                .map(|(v, p)| if *p > 0.0 { *v } else { 0.0 })
                .collect()
        })
        .collect()
}

pub struct DepthUNet {
    pub c1: Conv,
    pub c2: Conv,
    pub c3: Conv,
    pub c4: Conv,
    pub c5: Conv,
    pub c6: Conv,
    pub c7: Conv,
    pub c8: Conv,
    pub out_b: f32,
    pub med: f32,
    pub iqr: f32,
}

impl DepthUNet {
    /// `cin` 个输入平面（RGB [+ pyr/ppyr]），`h` 支路宽度（2·h 用于降采样支路）。
    pub fn new(cin: usize, h: usize, seed: u64, ternary: bool) -> Self {
        let mk = |ci: usize, co: usize, s: u64, tern: bool| {
            let mut c = Conv::new(ci, co, s);
            c.ternary = tern;
            c
        };
        let mut c8 = Conv::new(h, 1, seed ^ 0x9E37);
        c8.ternary = false;
        c8.scale.fill(0.5);
        Self {
            c1: mk(cin, h, seed, ternary),
            c2: mk(h, h, seed ^ 0x11, ternary),
            c3: mk(h, 2 * h, seed ^ 0x22, ternary),
            c4: mk(2 * h, 2 * h, seed ^ 0x33, ternary),
            c5: mk(2 * h, 2 * h, seed ^ 0x44, ternary),
            c6: mk(2 * h, 2 * h, seed ^ 0x55, ternary),
            c7: mk(2 * h, h, seed ^ 0x66, ternary),
            c8,
            out_b: 0.0,
            med: 0.0,
            iqr: 1.0,
        }
    }

    fn forward_cache(&self, x: &[Vec<f32>], h: usize, w: usize) -> (Vec<f32>, Cache) {
        let o1 = self.c1.forward(x, w, h);
        let r1 = relu(o1.clone());
        let o2 = self.c2.forward(&r1, w, h);
        let r2 = relu(o2.clone());
        let (h2, w2) = (h / 2, w / 2);
        // 逐通道池化（2×2 平均）
        let p1: Vec<Vec<f32>> = (0..r2.len()).map(|c| avgpool2(&r2[c], h, w).0).collect();
        let o3 = self.c3.forward(&p1, w2, h2);
        let r3 = relu(o3.clone());
        let o4 = self.c4.forward(&r3, w2, h2);
        let r4 = relu(o4.clone());
        let p2: Vec<Vec<f32>> = (0..r4.len()).map(|c| avgpool2(&r4[c], h2, w2).0).collect();
        let (h4, w4) = (h2 / 2, w2 / 2);
        let o5 = self.c5.forward(&p2, w4, h4);
        let r5 = relu(o5.clone());
        let u1: Vec<Vec<f32>> = (0..r5.len()).map(|c| up2(&r5[c], h4, w4).0).collect();
        let s1 = add_planes(&u1, &r4);
        let o6 = self.c6.forward(&s1, w2, h2);
        let r6 = relu(o6.clone());
        let u2: Vec<Vec<f32>> = (0..r6.len()).map(|c| up2(&r6[c], h2, w2).0).collect();
        let o7 = self.c7.forward(&u2, w, h);
        let r7 = relu(o7.clone());
        let o8 = self.c8.forward(&r7, w, h);
        let mut out = o8[0].clone();
        for v in &mut out {
            *v += self.out_b;
        }
        let cache = Cache {
            o1,
            r1,
            o2,
            p1,
            o3,
            r3,
            o4,
            p2,
            o5,
            s1,
            o6,
            u2,
            o7,
            r7,
        };
        (out, cache)
    }

    pub fn forward(&self, x: &[Vec<f32>], w: usize, h: usize) -> Vec<f32> {
        self.forward_cache(x, h, w).0
    }

    /// 一步训练（与 `DepthNet::train_step` 同口径：逐图归一化目标的 L2 + 掩码）。
    pub fn train_step(
        &mut self,
        x: &[Vec<f32>],
        w: usize,
        h: usize,
        target: &[f32],
        mask_: &[bool],
        lr: f32,
    ) -> f32 {
        let (pred, c) = self.forward_cache(x, h, w);
        let mut loss = 0.0f32;
        let mut valid = 0usize;
        let mut dout = vec![0.0f32; w * h];
        let mut gb = 0.0f32;
        for i in 0..w * h {
            if !mask_[i] {
                continue;
            }
            let d = pred[i] - target[i];
            loss += d * d;
            valid += 1;
            dout[i] = 2.0 * d;
            gb += 2.0 * d;
        }
        let inv = 1.0 / valid.max(1) as f32;
        for v in &mut dout {
            *v *= inv;
        }
        self.out_b -= lr * gb * inv;

        let (h2, w2) = (h / 2, w / 2);
        let (h4, w4) = (h2 / 2, w2 / 2);

        // c8 → c7
        let (d7, dw8, db8, ds8) = self.c8.backward(&c.r7, w, h, &[dout]);
        self.c8.apply(&dw8, &db8, &ds8, lr, true);
        let d7 = mask(&d7, &c.o7);
        let (du2, dw7, db7, ds7) = self.c7.backward(&c.u2, w, h, &d7);
        self.c7.apply(&dw7, &db7, &ds7, lr, true);
        // up2 → c6
        let dh5: Vec<Vec<f32>> = (0..du2.len()).map(|ch| up2_bwd(&du2[ch], h2, w2)).collect();
        let d5 = mask(&dh5, &c.o6);
        let (ds1, dw6, db6, ds6) = self.c6.backward(&c.s1, w2, h2, &d5);
        self.c6.apply(&dw6, &db6, &ds6, lr, true);
        // add 跳连：梯度同时流向 u1 与 r4
        let du1 = ds1.clone();
        let mut d4_add = ds1;
        // up2 → c5
        let dh4: Vec<Vec<f32>> = (0..du1.len()).map(|ch| up2_bwd(&du1[ch], h4, w4)).collect();
        let d4 = mask(&dh4, &c.o5);
        let (dp2, dw5, db5, ds5) = self.c5.backward(&c.p2, w4, h4, &d4);
        self.c5.apply(&dw5, &db5, &ds5, lr, true);
        // pool 反向 → 与跳连梯度相加
        for (ch, g) in dp2.iter().enumerate() {
            let back = avgpool2_bwd(g, h2, w2);
            for (a, b) in d4_add[ch].iter_mut().zip(&back) {
                *a += *b;
            }
        }
        let d4m = mask(&d4_add, &c.o4);
        let (d3, dw4, db4, ds4) = self.c4.backward(&c.r3, w2, h2, &d4m);
        self.c4.apply(&dw4, &db4, &ds4, lr, true);
        let d3 = mask(&d3, &c.o3);
        let (dp1, dw3, db3, ds3) = self.c3.backward(&c.p1, w2, h2, &d3);
        self.c3.apply(&dw3, &db3, &ds3, lr, true);
        let dh1: Vec<Vec<f32>> = (0..dp1.len())
            .map(|ch| avgpool2_bwd(&dp1[ch], h, w))
            .collect();
        let d2 = mask(&dh1, &c.o2);
        let (dh0, dw2, db2, ds2) = self.c2.backward(&c.r1, w, h, &d2);
        self.c2.apply(&dw2, &db2, &ds2, lr, true);
        let d1 = mask(&dh0, &c.o1);
        let (_dx, dw1, db1, ds1_c) = self.c1.backward(x, w, h, &d1);
        self.c1.apply(&dw1, &db1, &ds1_c, lr, true);
        loss * inv
    }

    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        use std::fmt::Write as _;
        let mut s = format!(
            "U1 {} {} {} {} {}\n",
            self.med, self.iqr, self.c1.cin, self.c1.cout, self.out_b
        );
        for c in [
            &self.c1, &self.c2, &self.c3, &self.c4, &self.c5, &self.c6, &self.c7, &self.c8,
        ] {
            let _ = writeln!(s, "{} {}", c.cin, c.cout);
            for v in [&c.w, &c.b, &c.scale] {
                for x in v.iter() {
                    let _ = write!(s, "{x} ");
                }
                let _ = writeln!(s);
            }
        }
        std::fs::write(path, s).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let txt = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut it = txt.split_whitespace();
        if it.next() != Some("U1") {
            return Err(format!("{}: 不是 U1", path.display()));
        }
        let med: f32 = it.next().and_then(|x| x.parse().ok()).ok_or("缺 med")?;
        let iqr: f32 = it.next().and_then(|x| x.parse().ok()).ok_or("缺 iqr")?;
        let cin: usize = it.next().and_then(|x| x.parse().ok()).ok_or("缺 cin")?;
        let hidden: usize = it.next().and_then(|x| x.parse().ok()).ok_or("缺 hidden")?;
        let out_b: f32 = it.next().and_then(|x| x.parse().ok()).ok_or("缺 out_b")?;
        let mut net = Self::new(cin, hidden, 1, true);
        for c in [
            &mut net.c1,
            &mut net.c2,
            &mut net.c3,
            &mut net.c4,
            &mut net.c5,
            &mut net.c6,
            &mut net.c7,
            &mut net.c8,
        ] {
            let ci: usize = it.next().ok_or("缺 cin")?.parse().map_err(|_| "cin")?;
            let co: usize = it.next().ok_or("缺 cout")?.parse().map_err(|_| "cout")?;
            let mut fresh = Conv::new(ci, co, 1);
            for v in fresh.w.iter_mut() {
                *v = it.next().and_then(|x| x.parse().ok()).ok_or("缺 w")?;
            }
            for v in fresh.b.iter_mut() {
                *v = it.next().and_then(|x| x.parse().ok()).ok_or("缺 b")?;
            }
            for v in fresh.scale.iter_mut() {
                *v = it.next().and_then(|x| x.parse().ok()).ok_or("缺 scale")?;
            }
            fresh.ternary = c.ternary;
            *c = fresh;
        }
        net.med = med;
        net.iqr = iqr;
        net.out_b = out_b;
        Ok(net)
    }
}

struct Cache {
    o1: Vec<Vec<f32>>,
    r1: Vec<Vec<f32>>,
    o2: Vec<Vec<f32>>,
    p1: Vec<Vec<f32>>,
    o3: Vec<Vec<f32>>,
    r3: Vec<Vec<f32>>,
    o4: Vec<Vec<f32>>,
    p2: Vec<Vec<f32>>,
    o5: Vec<Vec<f32>>,
    s1: Vec<Vec<f32>>,
    o6: Vec<Vec<f32>>,
    u2: Vec<Vec<f32>>,
    o7: Vec<Vec<f32>>,
    r7: Vec<Vec<f32>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 池化/上采样的**伴随**恒等式：`<pool(x), g> == <x, pool_bwd(g)>`。
    #[test]
    fn pool_and_up_are_adjoint() {
        let (h, w) = (4, 4);
        let x: Vec<f32> = (0..h * w).map(|i| i as f32 * 0.5 - 3.0).collect();
        let (p, oh, ow) = avgpool2(&x, h, w);
        let g: Vec<f32> = (0..oh * ow).map(|i| i as f32 * 0.25 - 1.0).collect();
        let lhs: f32 = p.iter().zip(&g).map(|(a, b)| a * b).sum();
        let back = avgpool2_bwd(&g, h, w);
        let rhs: f32 = x.iter().zip(&back).map(|(a, b)| a * b).sum();
        assert!((lhs - rhs).abs() < 1e-5, "{lhs} vs {rhs}");

        let (u, uh, uw) = up2(&x, h, w);
        let g2: Vec<f32> = (0..uh * uw).map(|i| i as f32 * 0.1 - 2.0).collect();
        let lhs: f32 = u.iter().zip(&g2).map(|(a, b)| a * b).sum();
        let back = up2_bwd(&g2, h, w);
        let rhs: f32 = x.iter().zip(&back).map(|(a, b)| a * b).sum();
        assert!((lhs - rhs).abs() < 1e-4, "{lhs} vs {rhs}");
    }

    /// 形状：4×4 → pool 2×2（块均值）→ up 4×4（块复制）。
    #[test]
    fn pool_up_shapes() {
        let x: Vec<f32> = (0..16).map(|i| i as f32).collect();
        let (p, oh, ow) = avgpool2(&x, 4, 4);
        assert_eq!((oh, ow), (2, 2));
        assert!((p[0] - 2.5).abs() < 1e-6 && (p[3] - 12.5).abs() < 1e-6);
        let (u, uh, uw) = up2(&p, oh, ow);
        assert_eq!((uh, uw), (4, 4));
        assert!((u[0] - 2.5).abs() < 1e-6 && (u[15] - 12.5).abs() < 1e-6);
    }

    /// 单样本过拟合：8×8、h=2、300 步后 MSE 应显著下降（网络能学）。
    #[test]
    fn overfits_one_sample() {
        let (h, w) = (8, 8);
        let mut net = DepthUNet::new(3, 2, 7, true);
        let x: Vec<Vec<f32>> = (0..3)
            .map(|c| {
                (0..h * w)
                    .map(|i| ((i + c) % 7) as f32 / 3.5 - 1.0)
                    .collect()
            })
            .collect();
        let target: Vec<f32> = (0..h * w)
            .map(|i| if i % w < w / 2 { -0.5 } else { 0.5 })
            .collect();
        let mask = vec![true; h * w];
        let first = net.train_step(&x, w, h, &target, &mask, 0.02);
        for _ in 0..300 {
            net.train_step(&x, w, h, &target, &mask, 0.02);
        }
        let pred = net.forward(&x, w, h);
        let mse: f32 = pred
            .iter()
            .zip(&target)
            .map(|(a, b)| (a - b) * (a - b))
            .sum::<f32>()
            / (h * w) as f32;
        assert!(first > 0.1, "起始误差 {first} 太小，夹具没区分度");
        assert!(mse < first * 0.6, "300 步后 {mse} 应显著低于 {first}");
    }
}
