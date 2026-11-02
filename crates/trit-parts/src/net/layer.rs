use wgpu::Buffer;

use crate::backward::ste_apply;
use crate::gpu::Gpu;
use crate::ste::Ste;
use crate::weights::ConvW;

/// 层序号（与 `forward` 的顺序一致）。
pub const E1A: usize = 0;
pub const E1B: usize = 1;
pub const E2A: usize = 2;
pub const E2B: usize = 3;
pub const E3A: usize = 4;
pub const E3B: usize = 5;
pub const U3: usize = 6;
pub const D2A: usize = 7;
pub const D2B: usize = 8;
pub const U2: usize = 9;
pub const D1A: usize = 10;
pub const D1B: usize = 11;
pub const HEAD: usize = 12;
/// 可选深度头（独立的第 14 层，从 d1 特征 `z` 分叉；存在时 `layers.len() == 14`）。
pub const DEPTH: usize = 13;

/// 一层的可训练参数与（可选的）设备缓冲。
pub struct Layer {
    pub latent: Vec<f32>,
    pub scale: Vec<f32>,
    pub q: Vec<i32>,
    pub bias: Vec<f32>,
    pub cin: u32,
    pub cout: u32,
    pub kk: u32,
    pub relu: bool,
    /// `∂L/∂E` 与 `∂L/∂b`（一次反向的累加结果）。
    pub grad_e: Vec<f32>,
    pub grad_b: Vec<f32>,
    pub(crate) m_w: Vec<f32>,
    pub(crate) v_w: Vec<f32>,
    pub(crate) m_s: Vec<f32>,
    pub(crate) v_s: Vec<f32>,
    pub(crate) m_b: Vec<f32>,
    pub(crate) v_b: Vec<f32>,
    pub t: u32,
    pub(crate) dev: Option<Dev>,
    /// 设备侧梯度累加器（每 accum 个样本才读回一次，省掉逐样本同步）。
    pub(crate) acc_e: Option<Buffer>,
    pub(crate) acc_b: Option<Buffer>,
    /// per-channel affine（可学习 BN 的等价物，无 running stats）。
    pub gamma: Vec<f32>,
    pub beta: Vec<f32>,
    pub grad_gamma: Vec<f32>,
    pub grad_beta: Vec<f32>,
    pub(crate) m_g: Vec<f32>,
    pub(crate) v_g: Vec<f32>,
    pub(crate) m_be: Vec<f32>,
    pub(crate) v_be: Vec<f32>,
    pub(crate) dev_gb: Option<(Buffer, Buffer)>,
    pub(crate) acc_gb: Option<Buffer>,
    /// 转置后的有效权重（dx 用；只读，随 upload 重建）。
    pub(crate) dev_t: Option<Buffer>,
    /// 是否是转置卷积（决定转置布局）。
    pub convt: bool,
    /// 残差：`y = relu(γ·v+β) + x`（要求 cin == cout）。
    pub residual: bool,
}

pub(crate) struct Dev {
    wf: Buffer,
    wt: Buffer,
    ws: Buffer,
    b: Buffer,
}

impl Layer {
    /// 从推理 ckpt 的 `ConvW` 建层：`latent = q·scale`（与 ckpt 的三值前向逐位等价）。
    pub fn from_conv_w(cw: &ConvW, relu: bool) -> Self {
        let n = cw.wf.len();
        let slots = cw.cout as usize;
        let latent: Vec<f32> = (0..n)
            .map(|wi| cw.wt[wi] as f32 * cw.ws[wi % slots])
            .collect();
        Self {
            latent,
            scale: cw.ws.clone(),
            q: cw.wt.clone(),
            bias: cw.bias.clone(),
            cin: cw.cin,
            cout: cw.cout,
            kk: cw.kk,
            relu,
            grad_e: vec![0.0; n],
            grad_b: vec![0.0; slots],
            m_w: vec![0.0; n],
            v_w: vec![0.0; n],
            m_s: vec![0.0; slots],
            v_s: vec![0.0; slots],
            m_b: vec![0.0; slots],
            v_b: vec![0.0; slots],
            t: 0,
            dev: None,
            acc_e: None,
            acc_b: None,
            gamma: vec![1.0; slots],
            beta: vec![0.0; slots],
            grad_gamma: vec![0.0; slots],
            grad_beta: vec![0.0; slots],
            m_g: vec![0.0; slots],
            v_g: vec![0.0; slots],
            m_be: vec![0.0; slots],
            v_be: vec![0.0; slots],
            dev_gb: None,
            acc_gb: None,
            dev_t: None,
            convt: false,
            residual: false,
        }
    }

    /// 通道扩展：输出通道从 `cout` 扩到 `cout+extra`，旧通道原样保留，新通道权重为 0。
    ///
    /// 三值权重按 `[wb + ic*cout + oc]` 排布，扩通道必须重排，不能简单追加。
    pub fn expand_cout(&mut self, extra: u32) {
        if extra == 0 {
            return;
        }
        let (cin, kk) = (self.cin as usize, self.kk as usize);
        let old = self.cout as usize;
        let new = old + extra as usize;
        let wb_n = kk * kk * cin;
        let mut latent = vec![0f32; wb_n * new];
        let mut q = vec![0i32; wb_n * new];
        for wb in 0..wb_n {
            let o = wb * old;
            let n = wb * new;
            latent[n..n + old].copy_from_slice(&self.latent[o..o + old]);
            q[n..n + old].copy_from_slice(&self.q[o..o + old]);
        }
        let grow = |v: &[f32], fill: f32| {
            let mut out = v.to_vec();
            out.resize(new, fill);
            out
        };
        self.latent = latent;
        self.q = q;
        self.grad_e = vec![0.0; wb_n * new];
        self.m_w = vec![0.0; wb_n * new];
        self.v_w = vec![0.0; wb_n * new];
        self.scale = grow(&self.scale, 1.0);
        self.bias = grow(&self.bias, 0.0);
        self.grad_b = grow(&self.grad_b, 0.0);
        self.m_s = grow(&self.m_s, 0.0);
        self.v_s = grow(&self.v_s, 0.0);
        self.m_b = grow(&self.m_b, 0.0);
        self.v_b = grow(&self.v_b, 0.0);
        self.gamma = grow(&self.gamma, 1.0);
        self.beta = grow(&self.beta, 0.0);
        self.grad_gamma = grow(&self.grad_gamma, 0.0);
        self.grad_beta = grow(&self.grad_beta, 0.0);
        self.m_g = grow(&self.m_g, 0.0);
        self.v_g = grow(&self.v_g, 0.0);
        self.m_be = grow(&self.m_be, 0.0);
        self.v_be = grow(&self.v_be, 0.0);
        self.cout = new as u32;
        self.dev = None;
        self.dev_gb = None;
        self.dev_t = None;
        self.acc_e = None;
        self.acc_b = None;
        self.acc_gb = None;
    }

    /// 随机初始化（He 均匀：`U(-a, a)`，`a = sqrt(6 / fan_in)`）。
    ///
    /// 早期版本用 `U(-1.43, 1.43)` 不按 fan-in 缩放，从零训练时逐层放大到 1e10 量级（实测），
    /// 只能配 `--init` 用；现在按 He 缩放，从零训练可用。
    pub fn random(cin: u32, cout: u32, kk: u32, relu: bool, seed: &mut u64) -> Self {
        let n = (kk * kk * cin * cout) as usize;
        let fan_in = (cin * kk * kk) as f32;
        let a = (6.0 / fan_in.max(1.0)).sqrt();
        let mut latent = Vec::with_capacity(n);
        for _ in 0..n {
            *seed ^= *seed << 13;
            *seed ^= *seed >> 7;
            *seed ^= *seed << 17;
            let u = (*seed % 2001) as f32 / 2000.0; // [0,1]
            latent.push(a * (2.0 * u - 1.0));
        }
        let slots = cout as usize;
        let mut scale = vec![0f32; slots];
        for (oc, slot) in scale.iter_mut().enumerate() {
            let mut acc = 0f32;
            for (wi, w) in latent.iter().enumerate() {
                if wi % slots == oc {
                    acc += w.abs();
                }
            }
            *slot = (acc / (n / slots) as f32).max(1e-3);
        }
        let q: Vec<i32> = (0..n)
            .map(|wi| (latent[wi] / scale[wi % slots]).round().clamp(-1.0, 1.0) as i32)
            .collect();
        Self {
            latent,
            scale,
            q,
            bias: vec![0.0; slots],
            cin,
            cout,
            kk,
            relu,
            grad_e: vec![0.0; n],
            grad_b: vec![0.0; slots],
            m_w: vec![0.0; n],
            v_w: vec![0.0; n],
            m_s: vec![0.0; slots],
            v_s: vec![0.0; slots],
            m_b: vec![0.0; slots],
            v_b: vec![0.0; slots],
            t: 0,
            dev: None,
            acc_e: None,
            acc_b: None,
            gamma: vec![1.0; slots],
            beta: vec![0.0; slots],
            grad_gamma: vec![0.0; slots],
            grad_beta: vec![0.0; slots],
            m_g: vec![0.0; slots],
            v_g: vec![0.0; slots],
            m_be: vec![0.0; slots],
            v_be: vec![0.0; slots],
            dev_gb: None,
            acc_gb: None,
            dev_t: None,
            convt: false,
            residual: false,
        }
    }

    pub(crate) fn ensure_acc(&mut self, gpu: &Gpu, n_e: usize, n_b: usize) {
        if self.acc_e.is_none() {
            self.acc_e = Some(gpu.zeros(n_e, "acc-e"));
            self.acc_b = Some(gpu.zeros(n_b, "acc-b"));
        }
    }

    /// affine 参数的一步更新（与权重共用同一 Adam 步数 `t`）。
    pub fn update_affine(&mut self, lr: f32, adam: bool, grad_scale: f32) {
        if adam {
            let (b1, b2, eps) = (0.9f32, 0.999f32, 1e-8f32);
            let bc1 = 1.0 - b1.powi(self.t as i32);
            let bc2 = 1.0 - b2.powi(self.t as i32);
            let step = |p: &mut f32, g: f32, m: &mut f32, v: &mut f32| {
                *m = b1 * *m + (1.0 - b1) * g;
                *v = b2 * *v + (1.0 - b2) * g * g;
                *p -= lr * (*m / bc1) / ((*v / bc2).sqrt() + eps);
            };
            for oc in 0..self.gamma.len() {
                step(
                    &mut self.gamma[oc],
                    self.grad_gamma[oc] * grad_scale,
                    &mut self.m_g[oc],
                    &mut self.v_g[oc],
                );
                step(
                    &mut self.beta[oc],
                    self.grad_beta[oc] * grad_scale,
                    &mut self.m_be[oc],
                    &mut self.v_be[oc],
                );
            }
        } else {
            for oc in 0..self.gamma.len() {
                self.gamma[oc] -= lr * self.grad_gamma[oc] * grad_scale;
                self.beta[oc] -= lr * self.grad_beta[oc] * grad_scale;
            }
        }
        self.grad_gamma.fill(0.0);
        self.grad_beta.fill(0.0);
    }

    /// 把设备累加器里的梯度读回主机并清零（每个更新周期一次）。
    pub fn pull_grads(&mut self, gpu: &Gpu) {
        if let Some(b) = &self.acc_e {
            self.grad_e = gpu.read(b, self.latent.len());
            gpu.clear(b, self.latent.len());
        }
        if let Some(b) = &self.acc_b {
            self.grad_b = gpu.read(b, self.scale.len());
            gpu.clear(b, self.scale.len());
        }
    }

    /// `E = q·scale`（每输出通道一个 scale）。
    pub fn effective(&self) -> Vec<f32> {
        let slots = self.cout as usize;
        (0..self.latent.len())
            .map(|wi| self.q[wi] as f32 * self.scale[wi % slots])
            .collect()
    }

    /// 把参数推到设备（量化值变了就要重推）。
    pub fn upload(&mut self, gpu: &Gpu) {
        let eff = self.effective();
        let transposed = self.transposed(&eff);
        let ones = vec![1.0f32; self.cout as usize];
        match &mut self.dev {
            Some(d) => {
                gpu.write(&d.wf, &eff);
                gpu.write_i32(&d.wt, &self.q);
                gpu.write(&d.ws, &self.scale);
                gpu.write(&d.b, &self.bias);
                if let Some(t) = &self.dev_t {
                    gpu.write(t, &transposed);
                }
                if let Some((g, b)) = &self.dev_gb {
                    gpu.write(g, &self.gamma);
                    gpu.write(b, &self.beta);
                }
                let _ = ones;
            }
            None => {
                self.dev = Some(Dev {
                    wf: gpu.buf(&eff, "wf"),
                    wt: gpu.ibuf(&self.q, "wt"),
                    ws: gpu.buf(&self.scale, "ws"),
                    b: gpu.buf(&self.bias, "b"),
                });
                self.dev_t = Some(gpu.buf(&transposed, "we-t"));
                self.dev_gb = Some((gpu.buf(&self.gamma, "gamma"), gpu.buf(&self.beta, "beta")));
            }
        }
    }

    /// 把有效权重转成「oc 外、ic 内」布局（dx 与 vec4 前向共用）。
    fn transposed(&self, eff: &[f32]) -> Vec<f32> {
        if self.convt {
            crate::weights::transpose_convt(eff, self.cin as usize, self.cout as usize)
        } else {
            crate::weights::transpose_conv(
                eff,
                self.cin as usize,
                self.cout as usize,
                self.kk as usize,
            )
        }
    }

    pub fn affine_dev(&self) -> (&Buffer, &Buffer) {
        let (g, b) = self.dev_gb.as_ref().expect("层未上传");
        (g, b)
    }

    /// 把 affine 的 dγ/dβ 读回并清零。
    pub(crate) fn pull_affine_grads(&mut self, gpu: &Gpu) {
        let slots = self.scale.len();
        if let Some(b) = &self.acc_gb {
            let v = gpu.read(b, 2 * slots);
            self.grad_gamma.copy_from_slice(&v[..slots]);
            self.grad_beta.copy_from_slice(&v[slots..]);
            gpu.clear(b, 2 * slots);
        }
    }

    fn ensure_acc_gb(&mut self, gpu: &Gpu) {
        let slots = self.scale.len();
        if self.acc_gb.is_none() {
            self.acc_gb = Some(gpu.zeros(2 * slots, "acc-gb"));
        }
    }

    /// 累加一次 affine 的 dγ/dβ（设备侧）。
    pub fn acc_affine(&mut self, gpu: &Gpu, dgb: &Buffer) {
        let slots = self.scale.len();
        self.ensure_acc_gb(gpu);
        if let Some(acc) = &self.acc_gb {
            gpu.add_into(acc, dgb, 2 * slots);
        }
    }

    pub fn transposed_dev(&self) -> &Buffer {
        self.dev_t.as_ref().expect("层未上传")
    }

    pub fn dev(&self) -> (&Buffer, &Buffer, &Buffer, &Buffer) {
        let d = self.dev.as_ref().expect("层未上传");
        (&d.wf, &d.wt, &d.ws, &d.b)
    }

    /// 一步更新：STE 回落到 latent/scale，Adam（`adam = false` 时退化成 SGD）。
    #[allow(clippy::needless_range_loop)]
    pub fn update(&mut self, lr: f32, ste: Ste, adam: bool) {
        let (d_latent, d_scale) = ste_apply(
            &self.latent,
            &self.q,
            &self.scale,
            &self.grad_e,
            ste,
            self.cout as usize,
        );
        self.t += 1;
        if adam {
            let (b1, b2, eps) = (0.9f32, 0.999f32, 1e-8f32);
            let bc1 = 1.0 - b1.powi(self.t as i32);
            let bc2 = 1.0 - b2.powi(self.t as i32);
            let step = |p: &mut f32, g: f32, m: &mut f32, v: &mut f32| {
                *m = b1 * *m + (1.0 - b1) * g;
                *v = b2 * *v + (1.0 - b2) * g * g;
                *p -= lr * (*m / bc1) / ((*v / bc2).sqrt() + eps);
            };
            for wi in 0..self.latent.len() {
                step(
                    &mut self.latent[wi],
                    d_latent[wi],
                    &mut self.m_w[wi],
                    &mut self.v_w[wi],
                );
            }
            for oc in 0..self.scale.len() {
                step(
                    &mut self.scale[oc],
                    d_scale[oc],
                    &mut self.m_s[oc],
                    &mut self.v_s[oc],
                );
                step(
                    &mut self.bias[oc],
                    self.grad_b[oc],
                    &mut self.m_b[oc],
                    &mut self.v_b[oc],
                );
            }
            for s in &mut self.scale {
                *s = s.max(1e-4);
            }
        } else {
            for wi in 0..self.latent.len() {
                self.latent[wi] -= lr * d_latent[wi];
            }
            for oc in 0..self.scale.len() {
                self.scale[oc] = (self.scale[oc] - lr * d_scale[oc]).max(1e-4);
                self.bias[oc] -= lr * self.grad_b[oc];
            }
        }
        // 重新量化
        let slots = self.cout as usize;
        for wi in 0..self.latent.len() {
            let s = self.scale[wi % slots];
            self.q[wi] = (self.latent[wi] / s).round().clamp(-1.0, 1.0) as i32;
        }
        self.grad_e.fill(0.0);
        self.grad_b.fill(0.0);
    }
}
