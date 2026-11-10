//! 三值卷积材质分类头（A 类：直接用像素，不用手工特征）。
//!
//! 结构：`conv(3→8, 3×3, 三值+每通道 scale) → ReLU → conv(8→8) → ReLU → 全局平均 → 线性(8→4)`。
//! 全部权重三值（STE 反传，复用 [`crate::tconv::Conv`]），只有线性头是 f32。
//! 目的：和 §6.64 的手工特征 + 岭回归（留出 97.6%）做**同数据同协议**对照。

use crate::tconv::Conv;

/// 输入边长（正方形裁剪）。
pub const SIDE: usize = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatClass {
    Wood = 0,
    Metal = 1,
    Stone = 2,
    Fabric = 3,
}

impl MatClass {
    pub const ALL: [MatClass; 4] = [
        MatClass::Wood,
        MatClass::Metal,
        MatClass::Stone,
        MatClass::Fabric,
    ];
    pub fn name(self) -> &'static str {
        match self {
            MatClass::Wood => "wood",
            MatClass::Metal => "metal",
            MatClass::Stone => "stone",
            MatClass::Fabric => "fabric",
        }
    }
    pub fn from_name(s: &str) -> Option<MatClass> {
        MatClass::ALL.into_iter().find(|c| c.name() == s)
    }
}

/// 分类网络。`c1: 3→H`，`c2: H→H`，线性头 `H→4`。
pub struct MatNet {
    pub c1: Conv,
    pub c2: Conv,
    pub head_w: Vec<f32>,
    pub head_b: Vec<f32>,
    /// 训练时的 Adam 状态（线性头）
    mw: Vec<f32>,
    vw: Vec<f32>,
    mb: Vec<f32>,
    vb: Vec<f32>,
    t: u32,
}

struct Cache {
    h1: Vec<Vec<f32>>,
    h2: Vec<Vec<f32>>,
    /// ReLU 之后的 c1 输出（反传用）
    r1: Vec<Vec<f32>>,
    pooled: Vec<f32>,
}

impl MatNet {
    pub fn new(hidden: usize, seed: u64) -> Self {
        MatNet {
            c1: Conv::new(3, hidden, seed),
            c2: Conv::new(hidden, hidden, seed ^ 0x5DEECE66D),
            head_w: vec![0.0; hidden * 4],
            head_b: vec![0.0; 4],
            mw: vec![0.0; hidden * 4],
            vw: vec![0.0; hidden * 4],
            mb: vec![0.0; 4],
            vb: vec![0.0; 4],
            t: 0,
        }
    }

    fn hidden(&self) -> usize {
        self.c1.cout
    }

    fn forward(&self, x: &[Vec<f32>], w: usize, h: usize) -> Cache {
        let o1 = self.c1.forward(x, w, h);
        let r1: Vec<Vec<f32>> = o1
            .iter()
            .map(|m| m.iter().map(|v| v.max(0.0)).collect())
            .collect();
        let o2 = self.c2.forward(&r1, w, h);
        let h2: Vec<Vec<f32>> = o2
            .iter()
            .map(|m| m.iter().map(|v| v.max(0.0)).collect())
            .collect();
        let n = (w * h) as f32;
        let pooled: Vec<f32> = h2.iter().map(|m| m.iter().sum::<f32>() / n).collect();
        Cache {
            h1: o1,
            h2,
            r1,
            pooled,
        }
    }

    /// 前向给出 4 类 logits。
    pub fn logits(&self, x: &[Vec<f32>], w: usize, h: usize) -> [f32; 4] {
        let c = self.forward(x, w, h);
        self.head(&c.pooled)
    }

    fn head(&self, pooled: &[f32]) -> [f32; 4] {
        let hid = self.hidden();
        let mut out = [0.0f32; 4];
        for (k, o) in out.iter_mut().enumerate() {
            let mut s = self.head_b[k];
            for (j, p) in pooled.iter().enumerate().take(hid) {
                s += self.head_w[k * hid + j] * p;
            }
            *o = s;
        }
        out
    }

    /// 一步训练：CE 损失 + 反传（conv 用 STE 三值梯度，线性头只用自身梯度）。
    pub fn train_step(&mut self, x: &[Vec<f32>], w: usize, h: usize, label: usize, lr: f32) -> f32 {
        let hid = self.hidden();
        let c = self.forward(x, w, h);
        let logits = self.head(&c.pooled);
        // softmax + CE
        let mx = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let ex: Vec<f32> = logits.iter().map(|v| (v - mx).exp()).collect();
        let sum: f32 = ex.iter().sum();
        let prob: Vec<f32> = ex.iter().map(|v| v / sum).collect();
        let loss = -prob[label].max(1e-9).ln();
        // dL/dlogits
        let mut dl = prob;
        dl[label] -= 1.0;

        // 线性头：闭式梯度 + Adam
        self.t += 1;
        let (b1, b2, eps) = (0.9f32, 0.999f32, 1e-8f32);
        let c1a = 1.0 - b1.powi(self.t as i32);
        let c2a = 1.0 - b2.powi(self.t as i32);
        let mut dpool = vec![0.0f32; hid];
        for (k, g) in dl.iter().enumerate() {
            for (j, d) in dpool.iter_mut().enumerate() {
                let grad = *g * c.pooled[j];
                self.mw[k * hid + j] = b1 * self.mw[k * hid + j] + (1.0 - b1) * grad;
                self.vw[k * hid + j] = b2 * self.vw[k * hid + j] + (1.0 - b2) * grad * grad;
                self.head_w[k * hid + j] -=
                    lr * (self.mw[k * hid + j] / c1a) / ((self.vw[k * hid + j] / c2a).sqrt() + eps);
                *d += g * self.head_w[k * hid + j];
            }
            self.mb[k] = b1 * self.mb[k] + (1.0 - b1) * g;
            self.vb[k] = b2 * self.vb[k] + (1.0 - b2) * g * g;
            self.head_b[k] -= lr * (self.mb[k] / c1a) / ((self.vb[k] / c2a).sqrt() + eps);
        }

        // 全局平均池化的反传：dpool / (w*h) 均摊到每个位置
        let n = (w * h) as f32;
        let dh2: Vec<Vec<f32>> = dpool.iter().map(|g| vec![g / n; w * h]).collect();
        // ReLU 反传（c2 的输入是 r1，输出经 ReLU 得 h2）
        // 这里 dh2 是对 h2 的梯度；ReLU 在 h2 上，需按 h2>0 掩码
        let mut d2: Vec<Vec<f32>> = Vec::with_capacity(hid);
        for (j, g) in dh2.iter().enumerate() {
            let mut d = vec![0.0f32; w * h];
            for i in 0..w * h {
                if c.h2[j][i] > 0.0 {
                    d[i] = g[i];
                }
            }
            d2.push(d);
        }
        let (dr1, dw2, db2, dsc2) = self.c2.backward(&c.r1, w, h, &d2);
        self.c2.apply(&dw2, &db2, &dsc2, lr, true);
        let mut d1: Vec<Vec<f32>> = Vec::with_capacity(hid);
        for (j, g) in dr1.iter().enumerate() {
            let mut d = vec![0.0f32; w * h];
            for i in 0..w * h {
                if c.h1[j][i] > 0.0 {
                    d[i] = g[i];
                }
            }
            d1.push(d);
        }
        let (_dx, dw1, db1, dsc1) = self.c1.backward(x, w, h, &d1);
        self.c1.apply(&dw1, &db1, &dsc1, lr, true);
        loss
    }

    /// 存盘（文本；三值权重 + scale + 线性头）。
    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        use std::fmt::Write as _;
        let mut s = String::from("MCN1\n");
        for c in [&self.c1, &self.c2] {
            let _ = writeln!(s, "{} {}", c.cin, c.cout);
            for v in [&c.w, &c.b, &c.scale] {
                for x in v.iter() {
                    let _ = write!(s, "{x} ");
                }
                let _ = writeln!(s);
            }
        }
        for v in [&self.head_w, &self.head_b] {
            for x in v.iter() {
                let _ = write!(s, "{x} ");
            }
            let _ = writeln!(s);
        }
        std::fs::write(path, s).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let txt = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut it = txt.split_whitespace();
        if it.next() != Some("MCN1") {
            return Err(format!("{}: 不是 MCN1", path.display()));
        }
        let mut convs = Vec::new();
        for _ in 0..2 {
            let cin: usize = it
                .next()
                .ok_or("缺 cin")?
                .parse()
                .map_err(|_| "cin 解析失败")?;
            let cout: usize = it
                .next()
                .ok_or("缺 cout")?
                .parse()
                .map_err(|_| "cout 解析失败")?;
            let mut c = Conv::new(cin, cout, 1);
            for v in c.w.iter_mut() {
                *v = it.next().and_then(|x| x.parse().ok()).ok_or("缺 w")?;
            }
            for v in c.b.iter_mut() {
                *v = it.next().and_then(|x| x.parse().ok()).ok_or("缺 b")?;
            }
            for v in c.scale.iter_mut() {
                *v = it.next().and_then(|x| x.parse().ok()).ok_or("缺 scale")?;
            }
            convs.push(c);
        }
        let hid = convs[0].cout;
        let mut head_w = vec![0.0f32; hid * 4];
        for v in head_w.iter_mut() {
            *v = it.next().and_then(|x| x.parse().ok()).ok_or("缺 head_w")?;
        }
        let mut head_b = vec![0.0f32; 4];
        for v in head_b.iter_mut() {
            *v = it.next().and_then(|x| x.parse().ok()).ok_or("缺 head_b")?;
        }
        Ok(MatNet {
            c1: convs[0].clone(),
            c2: convs[1].clone(),
            head_w,
            head_b,
            mw: vec![0.0; hid * 4],
            vw: vec![0.0; hid * 4],
            mb: vec![0.0; 4],
            vb: vec![0.0; 4],
            t: 0,
        })
    }

    pub fn predict(&self, x: &[Vec<f32>], w: usize, h: usize) -> (usize, f32) {
        let l = self.logits(x, w, h);
        let mut best = (f32::NEG_INFINITY, 0usize);
        let mut second = f32::NEG_INFINITY;
        for (k, v) in l.iter().enumerate() {
            if *v > best.0 {
                second = best.0;
                best = (*v, k);
            } else if *v > second {
                second = *v;
            }
        }
        (best.1, best.0 - second)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(c: [f32; 3]) -> Vec<Vec<f32>> {
        (0..3).map(|k| vec![c[k]; SIDE * SIDE]).collect()
    }

    #[test]
    fn learns_two_trivially_separable_classes() {
        let mut net = MatNet::new(8, 7);
        // 红块 vs 蓝块，两分类（用 0/3 两个标签），训练 200 步应几乎全对
        for step in 0..300 {
            let (x, y) = if step % 2 == 0 {
                (solid([0.9, 0.2, 0.1]), 0)
            } else {
                (solid([0.1, 0.2, 0.9]), 3)
            };
            net.train_step(&x, SIDE, SIDE, y, 0.02);
        }
        let (a, _) = net.predict(&solid([0.9, 0.2, 0.1]), SIDE, SIDE);
        let (b, _) = net.predict(&solid([0.1, 0.2, 0.9]), SIDE, SIDE);
        assert_eq!(a, 0, "红色应判为 0，实际 {a}");
        assert_eq!(b, 3, "蓝色应判为 3，实际 {b}");
    }

    #[test]
    fn save_load_roundtrip_keeps_predictions() {
        let mut net = MatNet::new(6, 3);
        for step in 0..60 {
            let (x, y) = if step % 2 == 0 {
                (solid([0.8, 0.8, 0.1]), 0)
            } else {
                (solid([0.2, 0.6, 0.2]), 2)
            };
            net.train_step(&x, SIDE, SIDE, y, 0.01);
        }
        let p = std::env::temp_dir().join("matnet_roundtrip.mcn");
        net.save(&p).unwrap();
        let back = MatNet::load(&p).unwrap();
        let a = net.predict(&solid([0.8, 0.8, 0.1]), SIDE, SIDE);
        let b = back.predict(&solid([0.8, 0.8, 0.1]), SIDE, SIDE);
        assert_eq!(a.0, b.0);
        assert!((a.1 - b.1).abs() < 1e-4);
        std::fs::remove_file(p).ok();
    }

    #[test]
    fn training_is_deterministic() {
        let mut a = MatNet::new(4, 11);
        let mut b = MatNet::new(4, 11);
        for _ in 0..20 {
            let x = solid([0.5, 0.3, 0.2]);
            a.train_step(&x, SIDE, SIDE, 1, 0.01);
            b.train_step(&x, SIDE, SIDE, 1, 0.01);
        }
        assert_eq!(a.head_w, b.head_w);
        assert_eq!(a.c1.w, b.c1.w);
    }
}
