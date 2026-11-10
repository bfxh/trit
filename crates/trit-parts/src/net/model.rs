use wgpu::Buffer;

use crate::gpu::Gpu;
use crate::weights::{ConvW, Tensors, conv_w, convt_w};
use trit_learn::Ste;

use super::layer::*;

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
    /// 深度头的输出（没有深度头时为 None）。
    pub depth_out: Option<Buffer>,
}

/// 整网。
pub struct Net {
    pub layers: Vec<Layer>,
    /// head 的学习率倍率（深度头用：新通道从 0 起，需要更快的学习速度）。
    pub head_lr_mult: f32,
    /// 序号小于该值的层不更新（冻结骨干做深度头专用训练）。
    pub freeze_until: usize,
    /// 冻结分割 head（只适配特征 + 深度头时用）。
    pub freeze_head: bool,
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
            freeze_until: 0,
            freeze_head: false,
        })
    }

    /// 追加独立深度头：1×1 卷积，输入取 `D1B` 的输出，输出 `planes*3` 通道。
    pub fn add_depth_head(&mut self, planes: u32, seed: u64) {
        if self.layers.len() > DEPTH {
            return;
        }
        let cin = self.layers[D1B].cout;
        let mut s = seed | 1;
        // 输出通道要补到 4 的倍数（vec4 反向内核）；多出来的补零通道不参与损失。
        let need = planes * 3;
        let cout = need + (4 - need % 4) % 4;
        // 实测 1×1 在留出集上比 3×3 好（MAE 1.09/1.31 m vs 1.20/1.48 m，3000 步）。
        self.layers.push(Layer::random(cin, cout, 1, false, &mut s));
    }

    /// 深度头输出通道数（没有时 0）。
    pub fn depth_cout(&self) -> u32 {
        if self.layers.len() > DEPTH {
            self.layers[DEPTH].cout
        } else {
            0
        }
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
            freeze_until: 0,
            freeze_head: false,
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
        let depth_out = if self.layers.len() > DEPTH {
            let (v_depth, d_out) = self.conv_affine(gpu, DEPTH, &z, res, res);
            pre[DEPTH] = Some(v_depth);
            Some(d_out)
        } else {
            None
        };
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
            depth_out,
        }
    }

    /// 反向：`dout` 是 `∂L/∂out`（NHWC，16 通道）。结果累加到各层梯度缓冲。
    pub fn backward(&mut self, gpu: &Gpu, tr: &Trace, dout: &Buffer, res: u32) {
        self.backward_full(gpu, tr, dout, None, res);
    }

    /// 反向：主 head + 可选深度头，两路梯度在 `z` 处相加后继续回传。
    pub fn backward_full(
        &mut self,
        gpu: &Gpu,
        tr: &Trace,
        dout: &Buffer,
        dout_depth: Option<&Buffer>,
        res: u32,
    ) {
        let h2 = res / 2;
        let h4 = res / 4;
        let pre = |i: usize| tr.pre[i].as_ref().expect("缺预激活");

        let dv = self.affine_bwd(gpu, HEAD, pre(HEAD), dout, res, res);
        let dz_main = self.bwd_conv(gpu, HEAD, &tr.z, &dv, res, res);
        let dz = match dout_depth {
            Some(dd) => {
                let dv2 = self.affine_bwd(gpu, DEPTH, pre(DEPTH), dd, res, res);
                let dz2 = self.bwd_conv(gpu, DEPTH, &tr.z, &dv2, res, res);
                gpu.add(
                    &dz_main,
                    &dz2,
                    (res * res) as usize * self.layers[D1B].cout as usize,
                )
            }
            None => dz_main,
        };
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
            if i < self.freeze_until || (self.freeze_head && i == HEAD) {
                continue;
            }
            let lr = if i == HEAD || i == DEPTH {
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
