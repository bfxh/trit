//! 整网前向 / 反向 / 更新（QAT：三值权重 + STE）。
//!
//! 图与 `parts-parity` 的前向完全一致（BN 已折叠、无 BN 参数参与训练）：
//! `x → e1 → pool → e2 → pool → e3 → u3 → cat(b) → d2 → u2 → cat(a) → d1 → head`。
//! 反向只对**有效权重 E** 求导（GPU），`E → latent/scale` 用 `backward::ste_apply`（主机侧）。

use wgpu::Buffer;

use crate::backward::ste_apply;
use crate::gpu::Gpu;
use crate::weights::{ConvW, Tensors, conv_w, convt_w};
use trit_learn::Ste;

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

    fn ensure_acc(&mut self, gpu: &Gpu, n_e: usize, n_b: usize) {
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
    fn pull_affine_grads(&mut self, gpu: &Gpu) {
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

/// 前向的中间结果（反向要用）。
pub struct Trace {
    /// 每层 affine 之前的卷积输出（convt 层填卷积输出，未用）。
    pub pre: Vec<Option<Buffer>>,
    pub input: Buffer,
    pub act_e1a: Buffer,
    pub a: Buffer,
    pub p1: Buffer,
    pub act_e2a: Buffer,
    pub b: Buffer,
    pub p2: Buffer,
    pub act_e3a: Buffer,
    pub c: Buffer,
    pub u3: Buffer,
    pub cat1: Buffer,
    pub act_d2a: Buffer,
    pub y: Buffer,
    pub u2: Buffer,
    pub cat2: Buffer,
    pub act_d1a: Buffer,
    pub z: Buffer,
    pub out: Buffer,
}

/// 整网。
pub struct Net {
    pub layers: Vec<Layer>,
    /// head 的学习率倍率（深度头用：新通道从 0 起，需要更快的学习速度）。
    pub head_lr_mult: f32,
}

impl Net {
    /// 从推理 ckpt 建网（逐位等价于 ckpt 的三值前向）。
    pub fn from_tensors(t: &Tensors) -> Result<Self, String> {
        let cw = |w: &str, bn: Option<&str>, relu: bool| -> Result<ConvW, String> {
            conv_w(t, w, bn, relu)
        };
        let ct = |w: &str| convt_w(t, w);
        let layers = vec![
            Layer::from_conv_w(&cw("e1.0.weight", Some("e1.1"), true)?, true),
            Layer::from_conv_w(&cw("e1.3.weight", Some("e1.4"), true)?, true),
            Layer::from_conv_w(&cw("e2.0.weight", Some("e2.1"), true)?, true),
            Layer::from_conv_w(&cw("e2.3.weight", Some("e2.4"), true)?, true),
            Layer::from_conv_w(&cw("e3.0.weight", Some("e3.1"), true)?, true),
            Layer::from_conv_w(&cw("e3.3.weight", Some("e3.4"), true)?, true),
            Layer::from_conv_w(&ct("u3.weight")?, false),
            Layer::from_conv_w(&cw("d2.0.weight", Some("d2.1"), true)?, true),
            Layer::from_conv_w(&cw("d2.3.weight", Some("d2.4"), true)?, true),
            Layer::from_conv_w(&ct("u2.weight")?, false),
            Layer::from_conv_w(&cw("d1.0.weight", Some("d1.1"), true)?, true),
            Layer::from_conv_w(&cw("d1.3.weight", Some("d1.4"), true)?, true),
            Layer::from_conv_w(&cw("head.weight", None, false)?, false),
        ];
        let mut layers = layers;
        layers[U3].convt = true;
        layers[U2].convt = true;
        Ok(Self {
            layers,
            head_lr_mult: 1.0,
        })
    }

    /// head 追加 `extra` 个输出通道（深度平面头用）。
    pub fn expand_head(&mut self, extra: u32) {
        self.layers[HEAD].expand_cout(extra);
    }

    /// head 的输出通道数（16 + 深度平面数）。
    pub fn head_cout(&self) -> u32 {
        self.layers[HEAD].cout
    }

    /// 随机初始化（按宽度倍数缩放通道：1.0 ⇒ 32/64/128；1.5 ⇒ 48/96/192）。
    pub fn random_w(seed: u64, width: f32) -> Self {
        let mut s = seed | 1;
        let q = |c: u32| -> u32 {
            let v = (c as f32 * width).round() as u32;
            (v.div_ceil(4) * 4).max(4)
        };
        let (c32, c64, c128, out) = (q(32), q(64), q(128), 16u32);
        let mut mk = |ci, co, kk, relu| Layer::random(ci, co, kk, relu, &mut s);
        let layers = vec![
            mk(3, c32, 3, true),
            mk(c32, c32, 3, true),
            mk(c32, c64, 3, true),
            mk(c64, c64, 3, true),
            mk(c64, c128, 3, true),
            mk(c128, c128, 3, true),
            mk(c128, c64, 2, false),
            mk(c64 + c64, c64, 3, true), // d2a 的输入是 cat(u3|c64, b|c64)
            mk(c64, c64, 3, true),
            mk(c64, c32, 2, false),
            mk(c32 + c32, c32, 3, true), // d1a 的输入是 cat(u2|c32, a|c32)
            mk(c32, c32, 3, true),
            mk(c32, out, 1, false),
        ];
        let mut layers = layers;
        layers[U3].convt = true;
        layers[U2].convt = true;
        let mut net = Self {
            layers,
            head_lr_mult: 1.0,
        };
        net.mark_convt();
        net
    }

    /// 默认宽度（32/64/128）。
    pub fn random(seed: u64) -> Self {
        Self::random_w(seed, 1.0)
    }

    /// 开启/关闭块内残差（只对 `cin == cout` 的卷积层生效：e1b/e2b/e3b/d2b/d1b）。
    pub fn set_residual(&mut self, on: bool) {
        for i in [E1B, E2B, E3B, D2B, D1B] {
            let l = &mut self.layers[i];
            l.residual = on && l.cin == l.cout && !l.convt;
        }
    }

    /// 标记两个转置卷积层（读断点时用）。
    pub fn mark_convt(&mut self) {
        if self.layers.len() > U3 {
            self.layers[U3].convt = true;
        }
        if self.layers.len() > U2 {
            self.layers[U2].convt = true;
        }
    }

    pub fn upload_all(&mut self, gpu: &Gpu) {
        for l in &mut self.layers {
            l.upload(gpu);
        }
    }

    /// 卷积（不带 ReLU）+ per-channel affine：返回 (预激活 v, 激活后 y)。
    fn conv_affine(&self, gpu: &Gpu, i: usize, x: &Buffer, h: u32, w: u32) -> (Buffer, Buffer) {
        let l = &self.layers[i];
        let (wf, wt, ws, b) = l.dev();
        let v = if l.cout.is_multiple_of(4) {
            gpu.conv_t(x, h, w, &spec_of(l), false, wf, b)
        } else {
            gpu.conv(x, h, w, &spec_of(l), 1, 0, wf, wt, ws, b)
        };
        let (g, be) = l.affine_dev();
        let n = (h * w * l.cout) as usize;
        let mut y = gpu.affine_fwd(&v, g, be, n, l.cout, l.relu);
        if l.residual {
            y = gpu.add(&y, x, n);
        }
        (v, y)
    }

    fn convt(&self, gpu: &Gpu, i: usize, x: &Buffer, h: u32, w: u32) -> Buffer {
        let l = &self.layers[i];
        let (wf, wt, ws, b) = l.dev();
        gpu.convt(x, h, w, &spec_of(l), 1, wf, wt, ws, b)
    }

    /// 前向（三值模式 + per-channel affine），保留全部中间激活。
    pub fn forward(&self, gpu: &Gpu, input: Buffer, res: u32) -> Trace {
        let h2 = res / 2;
        let h4 = res / 4;
        let mut pre: Vec<Option<Buffer>> = (0..self.layers.len()).map(|_| None).collect();
        let (v_e1a, act_e1a) = self.conv_affine(gpu, E1A, &input, res, res);
        pre[E1A] = Some(v_e1a);
        let (v_e1b, a) = self.conv_affine(gpu, E1B, &act_e1a, res, res);
        pre[E1B] = Some(v_e1b);
        let p1 = gpu.pool(&a, res, res, self.layers[E1B].cout);
        let (v_e2a, act_e2a) = self.conv_affine(gpu, E2A, &p1, h2, h2);
        pre[E2A] = Some(v_e2a);
        let (v_e2b, b) = self.conv_affine(gpu, E2B, &act_e2a, h2, h2);
        pre[E2B] = Some(v_e2b);
        let p2 = gpu.pool(&b, h2, h2, self.layers[E2B].cout);
        let (v_e3a, act_e3a) = self.conv_affine(gpu, E3A, &p2, h4, h4);
        pre[E3A] = Some(v_e3a);
        let (v_e3b, c) = self.conv_affine(gpu, E3B, &act_e3a, h4, h4);
        pre[E3B] = Some(v_e3b);
        let u3 = self.convt(gpu, U3, &c, h4, h4);
        let cat1 = gpu.cat(
            &u3,
            &b,
            h2 * h2,
            self.layers[U3].cout,
            self.layers[E2B].cout,
        );
        let (v_d2a, act_d2a) = self.conv_affine(gpu, D2A, &cat1, h2, h2);
        pre[D2A] = Some(v_d2a);
        let (v_d2b, y) = self.conv_affine(gpu, D2B, &act_d2a, h2, h2);
        pre[D2B] = Some(v_d2b);
        let u2 = self.convt(gpu, U2, &y, h2, h2);
        let cat2 = gpu.cat(
            &u2,
            &a,
            res * res,
            self.layers[U2].cout,
            self.layers[E1B].cout,
        );
        let (v_d1a, act_d1a) = self.conv_affine(gpu, D1A, &cat2, res, res);
        pre[D1A] = Some(v_d1a);
        let (v_d1b, z) = self.conv_affine(gpu, D1B, &act_d1a, res, res);
        pre[D1B] = Some(v_d1b);
        let (v_head, out) = self.conv_affine(gpu, HEAD, &z, res, res);
        pre[HEAD] = Some(v_head);
        Trace {
            pre,
            input,
            act_e1a,
            a,
            p1,
            act_e2a,
            b,
            p2,
            act_e3a,
            c,
            u3,
            cat1,
            act_d2a,
            y,
            u2,
            cat2,
            act_d1a,
            z,
            out,
        }
    }

    /// 反向：`dout` 是 `∂L/∂out`（NHWC，16 通道）。结果累加到各层梯度缓冲。
    pub fn backward(&mut self, gpu: &Gpu, tr: &Trace, dout: &Buffer, res: u32) {
        let h2 = res / 2;
        let h4 = res / 4;
        let pre = |i: usize| tr.pre[i].as_ref().expect("缺预激活");

        let dv = self.affine_bwd(gpu, HEAD, pre(HEAD), dout, res, res);
        let dz = self.bwd_conv(gpu, HEAD, &tr.z, &dv, res, res);
        let dv = self.affine_bwd(gpu, D1B, pre(D1B), &dz, res, res);
        let d_act1 = self.bwd_conv(gpu, D1B, &tr.act_d1a, &dv, res, res);
        let dv = self.affine_bwd(gpu, D1A, pre(D1A), &d_act1, res, res);
        let d_cat2 = self.bwd_conv(gpu, D1A, &tr.cat2, &dv, res, res);
        let (d_u2, d_a_cat) = gpu.bwd_cat_dx(
            res * res,
            self.layers[U2].cout,
            self.layers[E1B].cout,
            &d_cat2,
        );

        let d_y = self.bwd_convt(gpu, U2, &tr.y, &d_u2, h2, h2);
        let dv = self.affine_bwd(gpu, D2B, pre(D2B), &d_y, h2, h2);
        let d_act2 = self.bwd_conv(gpu, D2B, &tr.act_d2a, &dv, h2, h2);
        let dv = self.affine_bwd(gpu, D2A, pre(D2A), &d_act2, h2, h2);
        let d_cat1 = self.bwd_conv(gpu, D2A, &tr.cat1, &dv, h2, h2);
        let (d_u3, d_b_cat) = gpu.bwd_cat_dx(
            h2 * h2,
            self.layers[U3].cout,
            self.layers[E2B].cout,
            &d_cat1,
        );

        let d_c = self.bwd_convt(gpu, U3, &tr.c, &d_u3, h4, h4);
        let dv = self.affine_bwd(gpu, E3B, pre(E3B), &d_c, h4, h4);
        let d_act3 = self.bwd_conv(gpu, E3B, &tr.act_e3a, &dv, h4, h4);
        let dv = self.affine_bwd(gpu, E3A, pre(E3A), &d_act3, h4, h4);
        let d_p2 = self.bwd_conv(gpu, E3A, &tr.p2, &dv, h4, h4);
        let d_b_pool = gpu.bwd_pool_dx(h2, h2, self.layers[E2B].cout, &tr.b, &d_p2);

        let d_b = gpu.add(
            &d_b_cat,
            &d_b_pool,
            (h2 * h2 * self.layers[E2B].cout) as usize,
        );
        let dv = self.affine_bwd(gpu, E2B, pre(E2B), &d_b, h2, h2);
        let d_act2e = self.bwd_conv(gpu, E2B, &tr.act_e2a, &dv, h2, h2);
        let dv = self.affine_bwd(gpu, E2A, pre(E2A), &d_act2e, h2, h2);
        let d_p1 = self.bwd_conv(gpu, E2A, &tr.p1, &dv, h2, h2);
        let d_a_pool = gpu.bwd_pool_dx(res, res, self.layers[E1B].cout, &tr.a, &d_p1);

        let d_a = gpu.add(
            &d_a_cat,
            &d_a_pool,
            (res * res * self.layers[E1B].cout) as usize,
        );
        let dv = self.affine_bwd(gpu, E1B, pre(E1B), &d_a, res, res);
        let d_act1e = self.bwd_conv(gpu, E1B, &tr.act_e1a, &dv, res, res);
        let dv = self.affine_bwd(gpu, E1A, pre(E1A), &d_act1e, res, res);
        let _dx = self.bwd_conv(gpu, E1A, &tr.input, &dv, res, res);
    }

    #[allow(clippy::too_many_arguments)]
    fn bwd_conv(&mut self, gpu: &Gpu, i: usize, x: &Buffer, dv: &Buffer, h: u32, w: u32) -> Buffer {
        let l = &self.layers[i];
        let (wf, _, _, _) = l.dev();
        let spec = spec_of(l);
        // dv 已经含 affine 的掩码与 γ；这里不再做 ReLU 掩码
        let dx = gpu.bwd_conv_dx(h, w, &spec, 0, dv, wf, l.transposed_dev());
        let dw = gpu.bwd_conv_dw_split(h, w, &spec, 0, x, dv);
        let db = gpu.bwd_conv_db_split(h, w, &spec, 0, dv);
        // 残差：`y = a + x` ⇒ dL/dx 还要加上 dL/dy
        let dx = if l.residual {
            gpu.add(&dx, dv, (h * w * l.cout) as usize)
        } else {
            dx
        };
        let layer = &mut self.layers[i];
        let (n_e, n_b) = (layer.latent.len(), layer.scale.len());
        layer.ensure_acc(gpu, n_e, n_b);
        let (ae, ab) = (layer.acc_e.as_ref().unwrap(), layer.acc_b.as_ref().unwrap());
        gpu.add_into(ae, &dw, n_e);
        gpu.add_into(ab, &db, n_b);
        dx
    }

    /// affine 反向：返回 `dL/dv`，并把 dγ/dβ 累加到设备缓冲。
    fn affine_bwd(
        &mut self,
        gpu: &Gpu,
        i: usize,
        v: &Buffer,
        dy: &Buffer,
        h: u32,
        w: u32,
    ) -> Buffer {
        let n = (h * w * self.layers[i].cout) as usize;
        let (cout, relu) = (self.layers[i].cout, self.layers[i].relu);
        let (gv, dgb) = {
            let l = &self.layers[i];
            let (g, b) = l.affine_dev();
            (
                gpu.affine_gv(v, g, b, dy, n, cout, relu),
                gpu.affine_grad(v, g, b, dy, n, cout, relu),
            )
        };
        self.layers[i].acc_affine(gpu, &dgb);
        gv
    }

    fn bwd_convt(
        &mut self,
        gpu: &Gpu,
        i: usize,
        x: &Buffer,
        dy: &Buffer,
        h: u32,
        w: u32,
    ) -> Buffer {
        let l = &self.layers[i];
        let spec = spec_of(l);
        let dx = gpu.bwd_convt_dx(h, w, &spec, 1, dy, l.transposed_dev());
        let dw = gpu.bwd_convt_dw(h, w, &spec, x, dy);
        let db = gpu.bwd_convt_db(h, w, &spec, dy);
        let layer = &mut self.layers[i];
        let (n_e, n_b) = (layer.latent.len(), layer.scale.len());
        layer.ensure_acc(gpu, n_e, n_b);
        let (ae, ab) = (layer.acc_e.as_ref().unwrap(), layer.acc_b.as_ref().unwrap());
        gpu.add_into(ae, &dw, n_e);
        gpu.add_into(ab, &db, n_b);
        dx
    }

    /// 一步参数更新：先从显存取回梯度（可带 1/accum 缩放），再 STE + Adam/SGD。
    pub fn update(&mut self, gpu: &Gpu, lr: f32, ste: Ste, adam: bool, grad_scale: f32) {
        for (i, l) in self.layers.iter_mut().enumerate() {
            let lr = if i == HEAD {
                lr * self.head_lr_mult
            } else {
                lr
            };
            l.pull_grads(gpu);
            l.pull_affine_grads(gpu);
            if grad_scale != 1.0 {
                for g in &mut l.grad_e {
                    *g *= grad_scale;
                }
                for g in &mut l.grad_b {
                    *g *= grad_scale;
                }
            }
            l.update(lr, ste, adam);
            l.update_affine(lr, adam, grad_scale);
            l.upload(gpu);
        }
    }
}

fn spec_of(l: &Layer) -> ConvW {
    ConvW {
        wf: Vec::new(),
        wt: Vec::new(),
        ws: Vec::new(),
        bias: Vec::new(),
        cin: l.cin,
        cout: l.cout,
        kk: l.kk,
    }
}
