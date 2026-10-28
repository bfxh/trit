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
        let ones = vec![1.0f32; self.cout as usize];
        match &mut self.dev {
            Some(d) => {
                gpu.write(&d.wf, &eff);
                gpu.write_i32(&d.wt, &self.q);
                gpu.write(&d.ws, &self.scale);
                gpu.write(&d.b, &self.bias);
                let _ = ones;
            }
            None => {
                self.dev = Some(Dev {
                    wf: gpu.buf(&eff, "wf"),
                    wt: gpu.ibuf(&self.q, "wt"),
                    ws: gpu.buf(&self.scale, "ws"),
                    b: gpu.buf(&self.bias, "b"),
                });
            }
        }
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
        Self { layers }
    }

    pub fn upload_all(&mut self, gpu: &Gpu) {
        for l in &mut self.layers {
            l.upload(gpu);
        }
    }

    fn conv(&self, gpu: &Gpu, i: usize, x: &Buffer, h: u32, w: u32) -> Buffer {
        let l = &self.layers[i];
        let (wf, wt, ws, b) = l.dev();
        gpu.conv(x, h, w, &spec_of(l), 1, l.relu as u32, wf, wt, ws, b)
    }

    fn convt(&self, gpu: &Gpu, i: usize, x: &Buffer, h: u32, w: u32) -> Buffer {
        let l = &self.layers[i];
        let (wf, wt, ws, b) = l.dev();
        gpu.convt(x, h, w, &spec_of(l), 1, wf, wt, ws, b)
    }

    /// 前向（三值模式），保留全部中间激活。
    pub fn forward(&self, gpu: &Gpu, input: Buffer, res: u32) -> Trace {
        let h2 = res / 2;
        let h4 = res / 4;
        let act_e1a = self.conv(gpu, E1A, &input, res, res);
        let a = self.conv(gpu, E1B, &act_e1a, res, res);
        let p1 = gpu.pool(&a, res, res, 32);
        let act_e2a = self.conv(gpu, E2A, &p1, h2, h2);
        let b = self.conv(gpu, E2B, &act_e2a, h2, h2);
        let p2 = gpu.pool(&b, h2, h2, 64);
        let act_e3a = self.conv(gpu, E3A, &p2, h4, h4);
        let c = self.conv(gpu, E3B, &act_e3a, h4, h4);
        let u3 = self.convt(gpu, U3, &c, h4, h4);
        let cat1 = gpu.cat(&u3, &b, h2 * h2, 64, 64);
        let act_d2a = self.conv(gpu, D2A, &cat1, h2, h2);
        let y = self.conv(gpu, D2B, &act_d2a, h2, h2);
        let u2 = self.convt(gpu, U2, &y, h2, h2);
        let cat2 = gpu.cat(&u2, &a, res * res, 32, 32);
        let act_d1a = self.conv(gpu, D1A, &cat2, res, res);
        let z = self.conv(gpu, D1B, &act_d1a, res, res);
        let out = self.conv(gpu, HEAD, &z, res, res);
        Trace {
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

    /// 反向：`dout` 是 `∂L/∂out`（NHWC，16 通道）。结果累加到各层 `grad_e / grad_b`。
    pub fn backward(&mut self, gpu: &Gpu, tr: &Trace, dout: &Buffer, res: u32) {
        let h2 = res / 2;
        let h4 = res / 4;

        let dz = self.bwd_conv(gpu, HEAD, &tr.z, &tr.out, dout, res, res);
        let d_act1 = self.bwd_conv(gpu, D1B, &tr.act_d1a, &tr.z, &dz, res, res);
        let d_cat2 = self.bwd_conv(gpu, D1A, &tr.cat2, &tr.act_d1a, &d_act1, res, res);
        let (d_u2, d_a_cat) = gpu.bwd_cat_dx(res * res, 32, 32, &d_cat2);

        let d_y = self.bwd_convt(gpu, U2, &tr.y, &d_u2, h2, h2);
        let d_act2 = self.bwd_conv(gpu, D2B, &tr.act_d2a, &tr.y, &d_y, h2, h2);
        let d_cat1 = self.bwd_conv(gpu, D2A, &tr.cat1, &tr.act_d2a, &d_act2, h2, h2);
        let (d_u3, d_b_cat) = gpu.bwd_cat_dx(h2 * h2, 64, 64, &d_cat1);

        let d_c = self.bwd_convt(gpu, U3, &tr.c, &d_u3, h4, h4);
        let d_act3 = self.bwd_conv(gpu, E3B, &tr.p2, &tr.c, &d_c, h4, h4);
        let d_p2 = self.bwd_conv(gpu, E3A, &tr.p2, &tr.act_e3a, &d_act3, h4, h4);
        let d_b_pool = gpu.bwd_pool_dx(h2, h2, 64, &tr.b, &d_p2);

        let d_b = gpu.add(&d_b_cat, &d_b_pool, (h2 * h2 * 64) as usize);
        let d_act2e = self.bwd_conv(gpu, E2B, &tr.act_e2a, &tr.b, &d_b, h2, h2);
        let d_p1 = self.bwd_conv(gpu, E2A, &tr.p1, &tr.act_e2a, &d_act2e, h2, h2);
        let d_a_pool = gpu.bwd_pool_dx(res, res, 32, &tr.a, &d_p1);

        let d_a = gpu.add(&d_a_cat, &d_a_pool, (res * res * 32) as usize);
        let d_act1e = self.bwd_conv(gpu, E1B, &tr.act_e1a, &tr.a, &d_a, res, res);
        let _dx = self.bwd_conv(gpu, E1A, &tr.input, &tr.act_e1a, &d_act1e, res, res);
    }

    #[allow(clippy::too_many_arguments)]
    fn bwd_conv(
        &mut self,
        gpu: &Gpu,
        i: usize,
        x: &Buffer,
        y: &Buffer,
        dy: &Buffer,
        h: u32,
        w: u32,
    ) -> Buffer {
        let l = &self.layers[i];
        let (wf, wt, ws, _b) = l.dev();
        let spec = spec_of(l);
        let dx = gpu.bwd_conv_dx(h, w, &spec, 1, l.relu as u32, y, dy, wf, wt, ws);
        let dw = gpu.bwd_conv_dw(h, w, &spec, l.relu as u32, x, y, dy);
        let db = gpu.bwd_conv_db(h, w, &spec, l.relu as u32, y, dy);
        let layer = &mut self.layers[i];
        layer.grad_e = gpu.read(&dw, layer.grad_e.len());
        let dbv = gpu.read(&db, layer.grad_b.len());
        for (slot, v) in layer.grad_b.iter_mut().zip(dbv) {
            *slot += v;
        }
        dx
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
        let (wf, wt, ws, _b) = l.dev();
        let spec = spec_of(l);
        let dx = gpu.bwd_convt_dx(h, w, &spec, 1, dy, wf, wt, ws);
        let dw = gpu.bwd_convt_dw(h, w, &spec, x, dy);
        let db = gpu.bwd_convt_db(h, w, &spec, dy);
        let layer = &mut self.layers[i];
        layer.grad_e = gpu.read(&dw, layer.grad_e.len());
        let dbv = gpu.read(&db, layer.grad_b.len());
        for (slot, v) in layer.grad_b.iter_mut().zip(dbv) {
            *slot += v;
        }
        dx
    }

    /// 一步参数更新（STE + Adam/SGD），并重推量化值。
    pub fn update(&mut self, gpu: &Gpu, lr: f32, ste: Ste, adam: bool) {
        for l in &mut self.layers {
            l.update(lr, ste, adam);
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
    pub fn save_state(&self, path: &std::path::Path, step: u64) -> std::io::Result<()> {
        let mut b: Vec<u8> = Vec::new();
        b.extend_from_slice(b"TPNT1\0\0\0");
        b.extend_from_slice(&(self.layers.len() as u32).to_le_bytes());
        b.extend_from_slice(&step.to_le_bytes());
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
            b.extend_from_slice(&l.t.to_le_bytes());
        }
        std::fs::write(path, b)
    }

    /// 读断点，返回 `(网, step)`；层形状与当前代码不一致时报错（不猜）。
    pub fn load_state(path: &std::path::Path) -> Result<(Self, u64), String> {
        let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if b.len() < 16 || &b[0..8] != b"TPNT1\0\0\0" {
            return Err("不是 trit-parts 断点文件".into());
        }
        let mut at = 8usize;
        let n_layers = take_u32(&b, &mut at)? as usize;
        let step = take_u64(&b, &mut at)?;
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
                t,
                dev: None,
            });
        }
        Ok((Self { layers }, step))
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
