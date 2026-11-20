//! `PatchMlp`：3×3 补丁 MLP 去噪器（快，适合先把链路跑通）。
//!
//! `--patch` 是补丁边长，`nin = patch² + cond + 1` 随后进 `Mlp::new` 的
//! `nin×nhid`。`patch²` 是裸乘，`Mlp` 那边的 `checked_mul` 只挡 usize 绕回、
//! 挡不住「没溢出但量级已经错」——`--patch 65536` 给出 4.3e9 个特征，乘法干干净净地过。

use super::{Denoiser, add_noise};
use crate::occ::Mlp;

/// 补丁边长上限：3/5/7 是实际在用的档，`15²×256` 个 f32 才 230 KB。
pub const MAX_PATCH: usize = 15;

/// 补丁特征维校验，返回 `nin = patch² + cond + 1`。
///
/// `patch` 必须是奇数：`features` 里 `r = patch / 2`，循环是 `(2r+1)²` 个位置——
/// 偶数 `patch` 会让 `(2r+1)² ≠ patch²`，多出来的特征被 `Mlp::forward` 的 `take(nin)` 静默丢掉。
pub fn check(patch: usize, cond: usize) -> Result<usize, String> {
    if patch == 0 || patch.is_multiple_of(2) || patch > MAX_PATCH {
        return Err(format!(
            "patch 必须是 1..={MAX_PATCH} 内的奇数，实得 {patch}"
        ));
    }
    patch
        .checked_mul(patch)
        .and_then(|p| p.checked_add(cond))
        .and_then(|n| n.checked_add(1))
        .ok_or_else(|| format!("特征维 patch²+cond+1 溢出（{patch}² + {cond} + 1）"))
}

/// 3×3 补丁 MLP 去噪器（快，适合先把链路跑通）。
pub struct PatchMlp {
    pub mlp: Mlp,
    pub patch: usize,
    pub cond: usize,
}

impl PatchMlp {
    pub fn new(patch: usize, cond: usize, hid: usize, seed: u64) -> Result<Self, String> {
        let nin = check(patch, cond)?; // = patch² + cond + 1（+1 = t/T）
        Ok(PatchMlp {
            mlp: Mlp::new(nin, hid, seed)?,
            patch,
            cond,
        })
    }

    /// 特征：`xt` 的 patch + 条件 one-hot + t/T。
    pub fn features(
        &self,
        xt: &[f32],
        x: usize,
        y: usize,
        t_frac: f32,
        cond: Option<usize>,
        w: usize,
        h: usize,
    ) -> (Vec<f32>, f32) {
        assert!(
            xt.len() == w.saturating_mul(h),
            "xt 长度必须等于 w×h（{w}×{h}）"
        );
        let r = (self.patch / 2) as i32;
        let mut f = Vec::with_capacity(self.input_dim());
        for dy in -r..=r {
            for dx in -r..=r {
                let xx = ((x as i32 + dx).rem_euclid(w as i32)) as usize;
                let yy = ((y as i32 + dy).rem_euclid(h as i32)) as usize;
                f.push(xt[yy * w + xx]);
            }
        }
        for k in 0..self.cond {
            f.push((cond == Some(k)) as u8 as f32);
        }
        f.push(t_frac);
        (f, xt[y * w + x])
    }

    /// 一步训练 `k` 个像素（每个像素独立随机 t），返回平均 loss。
    pub fn train_shard(
        &mut self,
        x0: &[f32],
        abar: &[f32],
        cond: Option<usize>,
        w: usize,
        h: usize,
        lr: f32,
        k: usize,
        rng: &mut u64,
    ) -> f32 {
        let mut next = || {
            *rng ^= *rng << 13;
            *rng ^= *rng >> 7;
            *rng ^= *rng << 17;
            (*rng >> 11) as f32 / (1u64 << 53) as f32
        };
        let mut sum = 0.0f64;
        for _ in 0..k {
            let t = ((next() * (abar.len() - 1) as f32) as usize).min(abar.len() - 1);
            let eps: Vec<f32> = (0..crate::dims::pixels(w, h))
                .map(|_| {
                    let u1 = next().max(1e-6);
                    let u2 = next();
                    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
                })
                .collect();
            let xt = add_noise(x0, &eps, abar[t]);
            let tf = t as f32 / (abar.len() - 1).max(1) as f32;
            let p = ((next() * (w * h) as f32) as usize).min(w * h - 1);
            let (f, _) = self.features(&xt, p % w, p / w, tf, cond, w, h);
            sum += f64::from(self.mlp.train_step(&f, x0[p], lr));
        }
        (sum / k.max(1) as f64) as f32
    }

    /// 单步训练：随机时间步加噪 → 回归 x0。
    pub fn train_step(
        &mut self,
        x0: &[f32],
        abar: &[f32],
        cond: Option<usize>,
        w: usize,
        h: usize,
        lr: f32,
        rng: &mut u64,
    ) -> f32 {
        let mut next = || {
            *rng ^= *rng << 13;
            *rng ^= *rng >> 7;
            *rng ^= *rng << 17;
            (*rng >> 11) as f32 / (1u64 << 53) as f32
        };
        let t = ((next() * (abar.len() - 1) as f32) as usize).min(abar.len() - 1);
        let eps: Vec<f32> = (0..crate::dims::pixels(w, h))
            .map(|_| {
                let u1 = next().max(1e-6);
                let u2 = next();
                (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
            })
            .collect();
        let xt = add_noise(x0, &eps, abar[t]);
        let tf = t as f32 / (abar.len() - 1).max(1) as f32;
        // 随机取一个像素训练（每步便宜）
        let p = ((next() * (w * h) as f32) as usize).min(w * h - 1);
        let (f, _) = self.features(&xt, p % w, p / w, tf, cond, w, h);
        self.mlp.train_step(&f, x0[p], lr)
    }
}

impl Denoiser for PatchMlp {
    fn predict_x0(
        &self,
        xt: &[f32],
        t_frac: f32,
        cond: Option<usize>,
        w: usize,
        h: usize,
    ) -> Vec<f32> {
        let mut out = vec![0.0f32; crate::dims::pixels(w, h)];
        for y in 0..h {
            for x in 0..w {
                let (f, _) = self.features(xt, x, y, t_frac, cond, w, h);
                let (z, _) = self.mlp.forward(&f);
                out[y * w + x] = 1.0 / (1.0 + (-z).exp());
            }
        }
        out
    }

    fn input_dim(&self) -> usize {
        self.mlp.nin
    }
}
