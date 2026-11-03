//! 小型三值卷积**逐像素回归**头（用于单目深度/视差）。
//!
//! 结构：`conv(3→H,3×3) → ReLU → conv(H→H) → ReLU → conv(H→1,线性)`，输入/输出同分辨率。
//! 权重全三值（STE，复用 [`crate::tconv::Conv`]），只有最后 1×1 输出层也是三值 + 每通道 scale。
//!
//! 训练目标：**归一化视差**（每张图减中位、除 IQR），因此对全局尺度不敏感；
//! 推理时再按需反归一化（`--use-median-prior` 用训练集统计）。

use crate::tconv::Conv;

/// 一张目标图（值 + 有效掩码）。参数多时打包，避免 clippy 的 too_many_arguments。
pub struct TargetMap<'a> {
    pub y: &'a [f32],
    pub mask: &'a [bool],
}

/// 多视角一致性项：warp 目标 + 掩码 + 权重 + 截断阈值 tau。
pub struct MvExtra<'a> {
    pub y: &'a [f32],
    pub mask: &'a [bool],
    pub weight: f32,
    pub tau: f32,
}

pub struct DepthNet {
    pub c1: Conv,
    pub c2: Conv,
    pub c3: Conv,
    /// 输出层的**外置**偏置：`Conv` 的 bias 在 scale 乘法之内，学全局 DC 时会与 scale 打架
    /// （实测 scale 被推到 0.02、输出恒 0）。数据集级归一化必须能预测每图的绝对深度均值。
    pub out_b: f32,
    /// 训练时的目标归一化统计（log 深度中位/四分位距），用于推理反归一化。
    pub med: f32,
    pub iqr: f32,
}

impl DepthNet {
    pub fn new(hidden: usize, seed: u64) -> Self {
        Self::with_ternary(hidden, seed, true)
    }

    /// `ternary=false` 时整网 f32（连续几何量允许 f32；三值 STE 在真实数据上会塌，见 §6.86）。
    pub fn with_ternary(hidden: usize, seed: u64, ternary: bool) -> Self {
        Self::with_inputs(3, hidden, seed, ternary)
    }

    /// `cin=6/9`：`--pyr` 多尺度输入（RGB + 4× 低通 [+ 8× 低通]）。
    pub fn with_inputs(cin: usize, hidden: usize, seed: u64, ternary: bool) -> Self {
        let mut c1 = Conv::new(cin, hidden, seed);
        let mut c2 = Conv::new(hidden, hidden, seed ^ 0x5DEECE66D);
        let mut c3 = Conv::new(hidden, 1, seed ^ 0xA5A5);
        c1.ternary = ternary;
        c2.ternary = ternary;
        c3.ternary = false;
        c3.scale.fill(0.5); // 输出层 scale 初值放大：0.1 时梯度推不动（§6.86）
        DepthNet {
            c1,
            c2,
            c3,
            out_b: 0.0,
            med: 0.0,
            iqr: 1.0,
        }
    }

    /// 全局膨胀率：感受野 = 6·dil + 1（三层 3×3）。单图内景深依赖全局布局，5×5 不够。
    pub fn set_dilation(&mut self, d: usize) {
        let d = d.max(1);
        self.c1.dil = d;
        self.c2.dil = d;
        self.c3.dil = d;
    }

    pub fn forward(&self, x: &[Vec<f32>], w: usize, h: usize) -> Vec<f32> {
        let o1 = self.c1.forward(x, w, h);
        let r1: Vec<Vec<f32>> = o1
            .iter()
            .map(|m| m.iter().map(|v| v.max(0.0)).collect())
            .collect();
        let o2 = self.c2.forward(&r1, w, h);
        let r2: Vec<Vec<f32>> = o2
            .iter()
            .map(|m| m.iter().map(|v| v.max(0.0)).collect())
            .collect();
        self.c3
            .forward(&r2, w, h)
            .into_iter()
            .next()
            .map(|m| m.iter().map(|v| v + self.out_b).collect())
            .unwrap_or_default()
    }

    /// 一步训练：L2 损失（对归一化 log 深度），返回平均误差。
    pub fn train_step(
        &mut self,
        x: &[Vec<f32>],
        w: usize,
        h: usize,
        target: &[f32],
        mask: &[bool],
        lr: f32,
    ) -> f32 {
        self.train_step_mv(x, w, h, TargetMap { y: target, mask }, None, lr)
    }

    /// 多视角一致性版：在 GT 的 L2 之外，再叠一项 `(warp_target, warp_mask, weight)`。
    /// warp 目标来自**另一个视角**的预测投到本视角（stop-gradient，此处按常数处理），
    /// 两项都是同一输出的 L2 ⇒ 梯度直接相加，各按自己的有效像素数归一。
    pub fn train_step_mv(
        &mut self,
        x: &[Vec<f32>],
        w: usize,
        h: usize,
        gt: TargetMap<'_>,
        extra: Option<MvExtra<'_>>,
        lr: f32,
    ) -> f32 {
        let (target, mask) = (gt.y, gt.mask);
        let o1 = self.c1.forward(x, w, h);
        let r1: Vec<Vec<f32>> = o1
            .iter()
            .map(|m| m.iter().map(|v| v.max(0.0)).collect())
            .collect();
        let o2 = self.c2.forward(&r1, w, h);
        let r2: Vec<Vec<f32>> = o2
            .iter()
            .map(|m| m.iter().map(|v| v.max(0.0)).collect())
            .collect();
        let pred = self
            .c3
            .forward(&r2, w, h)
            .into_iter()
            .next()
            .map(|m| m.iter().map(|v| v + self.out_b).collect::<Vec<_>>())
            .unwrap_or_default();
        let mut loss = 0.0f32;
        let mut valid = 0usize;
        let mut dout = vec![vec![0.0f32; w * h]];
        let mut grad_b = 0.0f32;
        let mut grad_b_warped = 0.0f32;
        for i in 0..w * h {
            if !mask[i] {
                continue;
            }
            let d = pred[i] - target[i];
            loss += d * d;
            valid += 1;
            dout[0][i] = 2.0 * d;
            grad_b += 2.0 * d;
        }
        // 逐像素梯度必须按有效像素数归一：`Conv::apply` 的 scale 走普通 SGD
        // （不归一化时 step = lr·Σ像素 ≈ 40，scale 在 ±10 与 1e-3 之间振荡，输出直接爆到 1e4）
        let inv_gt = 1.0 / valid.max(1) as f32;
        for g in &mut dout[0] {
            *g *= inv_gt;
        }
        let mut extra_avg = 0.0f32;
        if let Some(MvExtra {
            y: t2,
            mask: m2,
            weight: wt,
            tau,
        }) = extra
            && wt > 0.0
        {
            let mut n2 = 0usize;
            let mut g2 = vec![0.0f32; w * h];
            let mut gb2 = 0.0f32;
            let mut e2 = 0.0f32;
            for i in 0..w * h {
                if !m2[i] {
                    continue;
                }
                let d = pred[i] - t2[i];
                // 软化门（Cauchy 型）：|d|≫tau 的遮挡像素权重趋 0，但在预测还差时**不直接排除**
                // ——硬截断会自锁：预测一旦离 warp 超过 tau，一致性项就永远不再起作用。
                let wgt = if tau > 0.0 {
                    1.0 / (1.0 + (d / tau) * (d / tau))
                } else {
                    1.0
                };
                e2 += wgt * d * d;
                n2 += 1;
                g2[i] = 2.0 * wt * wgt * d;
                gb2 += 2.0 * wt * wgt * d;
            }
            let inv2 = 1.0 / n2.max(1) as f32;
            for i in 0..w * h {
                dout[0][i] += g2[i] * inv2;
            }
            grad_b_warped += gb2 * inv2;
            extra_avg = e2 * inv2;
        }
        // 两项各按自己的有效像素数归一，不能把 warp 项再乘一次 inv_gt
        self.out_b -= lr * (grad_b * inv_gt + grad_b_warped);
        let (dr2, dw3, db3, ds3) = self.c3.backward(&r2, w, h, &dout);
        self.c3.apply(&dw3, &db3, &ds3, lr, true);
        // 先过 ReLU 掩码（r2 = ReLU(o2)），再喂给 c2 —— 之前把未掩码的梯度给了 c2，链是断的
        let mut d2: Vec<Vec<f32>> = Vec::with_capacity(dr2.len());
        for (j, g) in dr2.iter().enumerate() {
            let mut d = vec![0.0f32; w * h];
            for i in 0..w * h {
                if o2[j][i] > 0.0 {
                    d[i] = g[i];
                }
            }
            d2.push(d);
        }
        let (dr1, dw2, db2, ds2) = self.c2.backward(&r1, w, h, &d2);
        self.c2.apply(&dw2, &db2, &ds2, lr, true);
        let mut d1: Vec<Vec<f32>> = Vec::with_capacity(dr1.len());
        for (j, g) in dr1.iter().enumerate() {
            let mut d = vec![0.0f32; w * h];
            for i in 0..w * h {
                if o1[j][i] > 0.0 {
                    d[i] = g[i];
                }
            }
            d1.push(d);
        }
        let (_dx, dw1, db1, ds1) = self.c1.backward(x, w, h, &d1);
        self.c1.apply(&dw1, &db1, &ds1, lr, true);
        loss / valid.max(1) as f32 + extra_avg
    }

    /// 归一化视差 → 相对值；`med/iqr` 由训练集统计给出。
    pub fn denorm(&self, v: f32) -> f32 {
        v * self.iqr + self.med
    }

    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        use std::fmt::Write as _;
        let mut s = format!(
            "DCN3 {} {} {} {} {}\n",
            self.med,
            self.iqr,
            u8::from(self.c1.ternary),
            self.out_b,
            self.c1.dil
        );
        for c in [&self.c1, &self.c2, &self.c3] {
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
        let tag = it.next();
        if !matches!(tag, Some("DCN1") | Some("DCN2") | Some("DCN3")) {
            return Err(format!("{}: 不是 DCN1/DCN2/DCN3", path.display()));
        }
        let med: f32 = it.next().and_then(|x| x.parse().ok()).ok_or("缺 med")?;
        let iqr: f32 = it.next().and_then(|x| x.parse().ok()).ok_or("缺 iqr")?;
        let tern: bool = it.next().and_then(|x| x.parse::<u8>().ok()).unwrap_or(1) != 0;
        let out_b: f32 = if matches!(tag, Some("DCN2") | Some("DCN3")) {
            it.next().and_then(|x| x.parse().ok()).ok_or("缺 out_b")?
        } else {
            0.0
        };
        // DCN3 起存膨胀率：不存会让推理按 dil=1 跑，与训练（dil=2）不是同一个函数。
        let dil: usize = if tag == Some("DCN3") {
            it.next().and_then(|x| x.parse().ok()).ok_or("缺 dil")?
        } else {
            1
        };
        let mut convs = Vec::new();
        for _ in 0..3 {
            let cin: usize = it.next().ok_or("缺 cin")?.parse().map_err(|_| "cin")?;
            let cout: usize = it.next().ok_or("缺 cout")?.parse().map_err(|_| "cout")?;
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
        let mut c1 = convs[0].clone();
        let mut c2 = convs[1].clone();
        c1.ternary = tern;
        c2.ternary = tern;
        let mut c3 = convs[2].clone();
        c3.ternary = false; // 与 new() 一致：输出层是线性 f32
        let mut net = DepthNet {
            c1,
            c2,
            c3,
            out_b,
            med,
            iqr,
        };
        net.set_dilation(dil);
        Ok(net)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 目标 = 左半 −0.5 / 右半 +0.5 的阶跃（线性斜坡不行：线性函数过任何卷积核后
    /// 内部恒为常数，只能靠零填充边界「看到」坡度）。
    fn ramp_target(w: usize, h: usize) -> Vec<f32> {
        let mut v = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                v[y * w + x] = if x < w / 2 { -0.5 } else { 0.5 };
            }
        }
        v
    }

    fn gray(level: f32, w: usize, h: usize) -> Vec<Vec<f32>> {
        (0..3).map(|_| vec![level; w * h]).collect()
    }

    /// 输入：横向亮度阶跃（与目标同向）；常数输入无法产生空间目标。
    fn ramp_input(w: usize, h: usize) -> Vec<Vec<f32>> {
        let mut m = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                m[y * w + x] = if x < w / 2 { 0.0 } else { 1.0 };
            }
        }
        vec![m.clone(), m.clone(), m]
    }

    #[test]
    fn learns_a_simple_ramp() {
        let (w, h) = (16usize, 16usize);
        let t = ramp_target(w, h);
        let mut net = DepthNet::new(6, 3);
        // 输入亮度与目标相关：亮 → 视差大
        for _ in 0..1200 {
            net.train_step(&ramp_input(w, h), w, h, &t, &vec![true; w * h], 0.02);
        }
        // 预测应与目标正相关（不做绝对精度要求）
        let p = net.forward(&ramp_input(w, h), w, h);
        // 注意：必须按**左右**取均值；按扁平数组前后半会取成上下半（这是旧测试的错）
        let (mut l, mut r, mut nl, mut nr) = (0.0f32, 0.0f32, 0usize, 0usize);
        for y in 0..h {
            for x in 0..w {
                let v = p[y * w + x];
                if x < w / 2 {
                    l += v;
                    nl += 1;
                } else {
                    r += v;
                    nr += 1;
                }
            }
        }
        let (mean_l, mean_r) = (l / nl as f32, r / nr as f32);
        assert!(
            mean_r > mean_l + 0.5,
            "右侧应明显更大：{mean_r:.3} vs {mean_l:.3}"
        );
    }

    #[test]
    fn learns_constant_offset() {
        // 回归：外置偏置必须能学到常数目标。之前 bias 在 scale 乘法之内，
        // 优化器把 scale 推到 0，输出恒 0（整网塌成常数预测）。
        let (w, h) = (8usize, 8usize);
        let t = vec![2.0f32; w * h];
        let mut net = DepthNet::new(4, 3);
        for _ in 0..300 {
            net.train_step(&gray(0.5, w, h), w, h, &t, &vec![true; w * h], 0.02);
        }
        let p = net.forward(&gray(0.5, w, h), w, h);
        let mean = p.iter().sum::<f32>() / p.len() as f32;
        assert!(
            mean > 1.5,
            "应学到常数 2.0，实际 {mean:.3}（out_b {:.3}）",
            net.out_b
        );
    }

    #[test]
    fn forward_includes_external_bias() {
        // 部署走 forward：外置 out_b 必须真的加在输出上（bias 在 Conv 的 scale 乘法之内，
        // 少了这一句 scale 一缩就把 DC 吞掉）。
        let (w, h) = (8usize, 8usize);
        let mut net = DepthNet::new(4, 3);
        let mean_of = |n: &DepthNet| {
            let p = n.forward(&gray(0.5, w, h), w, h);
            p.iter().sum::<f32>() / p.len() as f32
        };
        let base = mean_of(&net);
        net.out_b = 3.5;
        let shifted = mean_of(&net);
        assert!(
            (shifted - base - 3.5).abs() < 1e-3,
            "forward 必须加外置 out_b：{base:.3} → {shifted:.3}"
        );
    }

    #[test]
    fn mv_gate_skips_outliers() {
        // GT 全 0；warp 目标是 10（远超 tau=0.5，属于遮挡/离群）⇒ 截断门必须把它全部跳过，
        // 否则预测会被拉走。没有门的实现会在这里涨到 O(1) 以上。
        let (w, h) = (8usize, 8usize);
        let zero = vec![0.0f32; w * h];
        let ones = vec![true; w * h];
        let bad = vec![10.0f32; w * h];
        let mut net = DepthNet::new(4, 3);
        for _ in 0..60 {
            net.train_step_mv(
                &gray(0.5, w, h),
                w,
                h,
                TargetMap {
                    y: &zero,
                    mask: &ones,
                },
                Some(MvExtra {
                    y: &bad,
                    mask: &ones,
                    weight: 1.0,
                    tau: 0.5,
                }),
                0.02,
            );
        }
        let p = net.forward(&gray(0.5, w, h), w, h);
        let mean = p.iter().sum::<f32>() / p.len() as f32;
        assert!(
            mean.abs() < 1.0,
            "离群 warp 目标应被截断门跳过，实际 {mean:.3}"
        );
    }

    #[test]
    fn mv_inlier_pulls_prediction() {
        // GT 全 0、warp 目标 0.8（在 tau 内）⇒ 两项各占一半权重，最优解 ≈ 0.4。
        // 这条挡住「一致性项根本没接进损失」的实现。
        let (w, h) = (8usize, 8usize);
        let zero = vec![0.0f32; w * h];
        let ones = vec![true; w * h];
        let good = vec![0.8f32; w * h];
        let mut net = DepthNet::new(4, 3);
        for _ in 0..400 {
            net.train_step_mv(
                &gray(0.5, w, h),
                w,
                h,
                TargetMap {
                    y: &zero,
                    mask: &ones,
                },
                Some(MvExtra {
                    y: &good,
                    mask: &ones,
                    weight: 1.0,
                    tau: 0.5,
                }),
                0.02,
            );
        }
        let p = net.forward(&gray(0.5, w, h), w, h);
        let mean = p.iter().sum::<f32>() / p.len() as f32;
        // 软门会把平衡点从 0.4 拉低（|d| 越大权重越小），但只要是「接上了」就应明显 > 0
        assert!(mean > 0.1, "tau 内的 warp 目标必须起作用，实际 {mean:.3}");
    }

    #[test]
    fn save_load_roundtrip() {
        let mut net = DepthNet::new(4, 5);
        let (w, h) = (8usize, 8usize);
        net.train_step(
            &gray(0.5, w, h),
            w,
            h,
            &vec![0.2f32; w * h],
            &vec![true; w * h],
            0.01,
        );
        net.med = 0.25;
        net.iqr = 0.5;
        net.out_b = -0.75;
        net.set_dilation(2);
        let p = std::env::temp_dir().join("dcn_roundtrip.dcn");
        net.save(&p).unwrap();
        let back = DepthNet::load(&p).unwrap();
        assert_eq!(back.med, 0.25);
        assert_eq!(back.iqr, 0.5);
        assert_eq!(back.out_b, -0.75);
        assert_eq!(
            back.c1.dil, 2,
            "膨胀率必须存进文件（否则推理与训练不是同一个函数）"
        );
        let a = net.forward(&gray(0.3, w, h), w, h);
        let b = back.forward(&gray(0.3, w, h), w, h);
        for (x, y) in a.iter().zip(&b) {
            assert!((x - y).abs() < 1e-5);
        }
        std::fs::remove_file(p).ok();
    }

    #[test]
    fn deterministic() {
        let (w, h) = (8usize, 8usize);
        let t = vec![0.1f32; w * h];
        let mut a = DepthNet::new(3, 9);
        let mut b = DepthNet::new(3, 9);
        for _ in 0..5 {
            a.train_step(&gray(0.4, w, h), w, h, &t, &vec![true; w * h], 0.01);
            b.train_step(&gray(0.4, w, h), w, h, &t, &vec![true; w * h], 0.01);
        }
        assert_eq!(
            a.forward(&gray(0.2, w, h), w, h),
            b.forward(&gray(0.2, w, h), w, h)
        );
    }
}
