use super::net::Net;

/// v-预测适配：`v ≡ √ᾱ·ε − √(1−ᾱ)·x0`，换算 `x̂0 = √ᾱ·x_t − √(1−ᾱ)·v̂`。
/// 高 t（ᾱ→0）时 x̂0 ≈ −v̂，**没有 1/√ᾱ**，这条参数化就是为稳定而生的。
pub struct VDen {
    pub net: Net,
    pub abar: Vec<f32>,
    /// 追加在 `[x_t, t/T]` 之后的常数条件图（如目标均值/对比）；空 = 无条件。
    pub extra: Vec<f32>,
    /// 是否把 x̂0 裁到 `[0,1]`；归一化（z-score）训练时必须为 false。
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
