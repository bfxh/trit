//! 整网前向 / 反向 / 更新（QAT：三值权重 + STE）。
//!
//! 图与 `parts-parity` 的前向完全一致（BN 已折叠、无 BN 参数参与训练）：
//! `x → e1 → pool → e2 → pool → e3 → u3 → cat(b) → d2 → u2 → cat(a) → d1 → head`。
//! 反向只对**有效权重 E** 求导（GPU），`E → latent/scale` 用 `backward::ste_apply`（主机侧）。

use wgpu::Buffer;

use crate::backward::ste_apply;
use crate::gpu::Gpu;
use crate::weights::{ConvW, Tensors, conv_w, convt_w};
use trit_train::Ste;

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
    m_w: Vec<f32>,
    v_w: Vec<f32>,
    m_s: Vec<f32>,
    v_s: Vec<f32>,
    m_b: Vec<f32>,
    v_b: Vec<f32>,
    pub t: u32,
    dev: Option<Dev>,
    /// 设备侧梯度累加器（每 accum 个样本才读回一次，省掉逐样本同步）。
    acc_e: Option<Buffer>,
    acc_b: Option<Buffer>,
    /// per-channel affine（可学习 BN 的等价物，无 running stats）。
    pub gamma: Vec<f32>,
    pub beta: Vec<f32>,
    pub grad_gamma: Vec<f32>,
    pub grad_beta: Vec<f32>,
    m_g: Vec<f32>,
    v_g: Vec<f32>,
    m_be: Vec<f32>,
    v_be: Vec<f32>,
    dev_gb: Option<(Buffer, Buffer)>,
    acc_gb: Option<Buffer>,
    /// 转置后的有效权重（dx 用；只读，随 upload 重建）。
    dev_t: Option<Buffer>,
    /// 是否是转置卷积（决定转置布局）。
    pub convt: bool,
}

struct Dev {
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
        }
    }

    /// 随机初始化（overfit 自检用）。
    pub fn random(cin: u32, cout: u32, kk: u32, relu: bool, seed: &mut u64) -> Self {
        let n = (kk * kk * cin * cout) as usize;
        let mut latent = Vec::with_capacity(n);
        for _ in 0..n {
            *seed ^= *seed << 13;
            *seed ^= *seed >> 7;
            *seed ^= *seed << 17;
            latent.push(((*seed % 2001) as f32 - 1000.0) / 700.0);
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
        Ok(Self { layers })
    }

    /// 随机初始化（形状与 ckpt 一致：3→32→32, 32→64→64, 64→128→128, 128→64, 64→32, 32→16）。
    pub fn random(seed: u64) -> Self {
        let mut s = seed | 1;
        let mut mk = |ci, co, kk, relu| Layer::random(ci, co, kk, relu, &mut s);
        let layers = vec![
            mk(3, 32, 3, true),
            mk(32, 32, 3, true),
            mk(32, 64, 3, true),
            mk(64, 64, 3, true),
            mk(64, 128, 3, true),
            mk(128, 128, 3, true),
            mk(128, 64, 2, false),
            mk(128, 64, 3, true),
            mk(64, 64, 3, true),
            mk(64, 32, 2, false),
            mk(64, 32, 3, true),
            mk(32, 32, 3, true),
            mk(32, 16, 1, false),
        ];
        let mut layers = layers;
        layers[U3].convt = true;
        layers[U2].convt = true;
        Self { layers }
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
        let y = gpu.affine_fwd(&v, g, be, n, l.cout, l.relu);
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
        let p1 = gpu.pool(&a, res, res, 32);
        let (v_e2a, act_e2a) = self.conv_affine(gpu, E2A, &p1, h2, h2);
        pre[E2A] = Some(v_e2a);
        let (v_e2b, b) = self.conv_affine(gpu, E2B, &act_e2a, h2, h2);
        pre[E2B] = Some(v_e2b);
        let p2 = gpu.pool(&b, h2, h2, 64);
        let (v_e3a, act_e3a) = self.conv_affine(gpu, E3A, &p2, h4, h4);
        pre[E3A] = Some(v_e3a);
        let (v_e3b, c) = self.conv_affine(gpu, E3B, &act_e3a, h4, h4);
        pre[E3B] = Some(v_e3b);
        let u3 = self.convt(gpu, U3, &c, h4, h4);
        let cat1 = gpu.cat(&u3, &b, h2 * h2, 64, 64);
        let (v_d2a, act_d2a) = self.conv_affine(gpu, D2A, &cat1, h2, h2);
        pre[D2A] = Some(v_d2a);
        let (v_d2b, y) = self.conv_affine(gpu, D2B, &act_d2a, h2, h2);
        pre[D2B] = Some(v_d2b);
        let u2 = self.convt(gpu, U2, &y, h2, h2);
        let cat2 = gpu.cat(&u2, &a, res * res, 32, 32);
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
        let (d_u2, d_a_cat) = gpu.bwd_cat_dx(res * res, 32, 32, &d_cat2);

        let d_y = self.bwd_convt(gpu, U2, &tr.y, &d_u2, h2, h2);
        let dv = self.affine_bwd(gpu, D2B, pre(D2B), &d_y, h2, h2);
        let d_act2 = self.bwd_conv(gpu, D2B, &tr.act_d2a, &dv, h2, h2);
        let dv = self.affine_bwd(gpu, D2A, pre(D2A), &d_act2, h2, h2);
        let d_cat1 = self.bwd_conv(gpu, D2A, &tr.cat1, &dv, h2, h2);
        let (d_u3, d_b_cat) = gpu.bwd_cat_dx(h2 * h2, 64, 64, &d_cat1);

        let d_c = self.bwd_convt(gpu, U3, &tr.c, &d_u3, h4, h4);
        let dv = self.affine_bwd(gpu, E3B, pre(E3B), &d_c, h4, h4);
        let d_act3 = self.bwd_conv(gpu, E3B, &tr.act_e3a, &dv, h4, h4);
        let dv = self.affine_bwd(gpu, E3A, pre(E3A), &d_act3, h4, h4);
        let d_p2 = self.bwd_conv(gpu, E3A, &tr.p2, &dv, h4, h4);
        let d_b_pool = gpu.bwd_pool_dx(h2, h2, 64, &tr.b, &d_p2);

        let d_b = gpu.add(&d_b_cat, &d_b_pool, (h2 * h2 * 64) as usize);
        let dv = self.affine_bwd(gpu, E2B, pre(E2B), &d_b, h2, h2);
        let d_act2e = self.bwd_conv(gpu, E2B, &tr.act_e2a, &dv, h2, h2);
        let dv = self.affine_bwd(gpu, E2A, pre(E2A), &d_act2e, h2, h2);
        let d_p1 = self.bwd_conv(gpu, E2A, &tr.p1, &dv, h2, h2);
        let d_a_pool = gpu.bwd_pool_dx(res, res, 32, &tr.a, &d_p1);

        let d_a = gpu.add(&d_a_cat, &d_a_pool, (res * res * 32) as usize);
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
        for l in &mut self.layers {
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

impl Net {
    /// 存断点（含 Adam 状态，可续跑）。布局：magic + 层数 + step + 每层参数。
    pub fn save_state(&self, path: &std::path::Path, step: u64, rng: u64) -> std::io::Result<()> {
        let mut b: Vec<u8> = Vec::new();
        b.extend_from_slice(b"TPNT3\0\0\0");
        b.extend_from_slice(&(self.layers.len() as u32).to_le_bytes());
        b.extend_from_slice(&step.to_le_bytes());
        b.extend_from_slice(&rng.to_le_bytes());
        for l in &self.layers {
            for v in [
                l.cin,
                l.cout,
                l.kk,
                l.relu as u32,
                l.latent.len() as u32,
                l.scale.len() as u32,
            ] {
                b.extend_from_slice(&v.to_le_bytes());
            }
            for v in &l.latent {
                b.extend_from_slice(&v.to_le_bytes());
            }
            for v in &l.scale {
                b.extend_from_slice(&v.to_le_bytes());
            }
            for v in &l.q {
                b.extend_from_slice(&v.to_le_bytes());
            }
            for v in &l.bias {
                b.extend_from_slice(&v.to_le_bytes());
            }
            for arr in [&l.m_w, &l.v_w] {
                for v in arr {
                    b.extend_from_slice(&v.to_le_bytes());
                }
            }
            for arr in [&l.m_s, &l.v_s, &l.m_b, &l.v_b] {
                for v in arr {
                    b.extend_from_slice(&v.to_le_bytes());
                }
            }
            for arr in [&l.gamma, &l.beta, &l.m_g, &l.v_g, &l.m_be, &l.v_be] {
                for v in arr {
                    b.extend_from_slice(&v.to_le_bytes());
                }
            }
            b.extend_from_slice(&l.t.to_le_bytes());
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, b)?;
        std::fs::rename(&tmp, path)
    }

    /// 读断点，返回 `(网, step)`；层形状与当前代码不一致时报错（不猜）。
    /// 读进程存档，返回 `(网, step, RNG)`；TPNT1/TPNT2 老存档没有 RNG（返回 None）。
    pub fn load_state(path: &std::path::Path) -> Result<(Self, u64, Option<u64>), String> {
        let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if b.len() < 8 {
            return Err("不是 trit-parts 断点文件".into());
        }
        // TPNT1 没有 affine；TPNT2 起带 γ/β 与其 Adam 状态
        let v3 = &b[0..8] == b"TPNT3\0\0\0";
        let v2 = v3 || &b[0..8] == b"TPNT2\0\0\0";
        if !v2 && &b[0..8] != b"TPNT1\0\0\0" {
            return Err("不是 trit-parts 断点文件".into());
        }
        let mut at = 8usize;
        let n_layers = take_u32(&b, &mut at)? as usize;
        let step = take_u64(&b, &mut at)?;
        let rng = if v3 {
            Some(take_u64(&b, &mut at)?)
        } else {
            None
        };
        let mut layers = Vec::with_capacity(n_layers);
        for _ in 0..n_layers {
            let cin = take_u32(&b, &mut at)?;
            let cout = take_u32(&b, &mut at)?;
            let kk = take_u32(&b, &mut at)?;
            let relu = take_u32(&b, &mut at)? != 0;
            let n = take_u32(&b, &mut at)? as usize;
            let slots = take_u32(&b, &mut at)? as usize;
            let mut latent = vec![0.0f32; n];
            for v in &mut latent {
                *v = f32::from_bits(take_u32(&b, &mut at)?);
            }
            let mut scale = vec![0.0f32; slots];
            for v in &mut scale {
                *v = f32::from_bits(take_u32(&b, &mut at)?);
            }
            let mut q = vec![0i32; n];
            for v in &mut q {
                *v = take_u32(&b, &mut at)? as i32;
            }
            let mut bias = vec![0.0f32; slots];
            for v in &mut bias {
                *v = f32::from_bits(take_u32(&b, &mut at)?);
            }
            let take_f = |count: usize, b: &[u8], at: &mut usize| -> Result<Vec<f32>, String> {
                let mut v = vec![0.0f32; count];
                for x in &mut v {
                    *x = f32::from_bits(take_u32(b, at)?);
                }
                Ok(v)
            };
            let m_w = take_f(n, &b, &mut at)?;
            let v_w = take_f(n, &b, &mut at)?;
            let m_s = take_f(slots, &b, &mut at)?;
            let v_s = take_f(slots, &b, &mut at)?;
            let m_b = take_f(slots, &b, &mut at)?;
            let v_b = take_f(slots, &b, &mut at)?;
            let (mut gamma, mut beta) = (vec![1.0f32; slots], vec![0.0f32; slots]);
            let (mut m_g, mut v_g) = (vec![0.0f32; slots], vec![0.0f32; slots]);
            let (mut m_be, mut v_be) = (vec![0.0f32; slots], vec![0.0f32; slots]);
            if v2 {
                for arr in [
                    &mut gamma, &mut beta, &mut m_g, &mut v_g, &mut m_be, &mut v_be,
                ] {
                    for v in arr.iter_mut() {
                        *v = f32::from_bits(take_u32(&b, &mut at)?);
                    }
                }
            }
            let t = take_u32(&b, &mut at)?;
            layers.push(Layer {
                latent,
                scale,
                q,
                bias,
                cin,
                cout,
                kk,
                relu,
                grad_e: vec![0.0; n],
                grad_b: vec![0.0; slots],
                m_w,
                v_w,
                m_s,
                v_s,
                m_b,
                v_b,
                gamma,
                beta,
                grad_gamma: vec![0.0; slots],
                grad_beta: vec![0.0; slots],
                m_g,
                v_g,
                m_be,
                v_be,
                dev_gb: None,
                acc_gb: None,
                t,
                dev: None,
                acc_e: None,
                acc_b: None,
                dev_t: None,
                convt: false,
            });
        }
        Ok((Self { layers }, step, rng))
    }
}

fn take_u32(b: &[u8], at: &mut usize) -> Result<u32, String> {
    let s = b.get(*at..*at + 4).ok_or("断点文件截断")?;
    *at += 4;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn take_u64(b: &[u8], at: &mut usize) -> Result<u64, String> {
    let s = b.get(*at..*at + 8).ok_or("断点文件截断")?;
    *at += 8;
    let mut a = [0u8; 8];
    a.copy_from_slice(s);
    Ok(u64::from_le_bytes(a))
}

impl Net {
    /// 梯度除以累加步数（多帧取平均）。
    pub fn scale_grads(&mut self, f: f32) {
        for l in &mut self.layers {
            for g in &mut l.grad_e {
                *g *= f;
            }
            for g in &mut l.grad_b {
                *g *= f;
            }
        }
    }
}
