//! 三值卷积深度头的 **GPU 训练**（复用 parts 的 wgpu conv/bwd 内核）。
//!
//! 参数口径与 CPU 的 [`crate::tconv::Conv`] 逐点一致：
//! `out = scale·(b + conv(q(w)))`。GPU 内核吃的是「有效权重」`E = p·scale`
//! （`p` = 三值 `q` 或 f32 `w`）与 `bias = scale·b`，所以前向逐点等价；
//! 反向后按 scale 还原，更新公式与 CPU 的 Adam(w,b) + SGD(scale) 相同。
//!
//! 3×3 的膨胀卷积用 `(2·dil+1)²` 的**零权核**表示（只有 `dy·dil, dx·dil` 位置非零），
//! 不改 shader、数值上与 CPU 的 `dil` 完全一致。
//!
//! 输出层 `cout=1` 补齐到 4（`bwd_conv_dx/dw` 要求 `cout % 4 == 0`），只读通道 0。

use wgpu::Buffer;

use crate::depthnet::DepthNet;
use crate::gpu::Gpu;
use crate::tconv::Conv;
use crate::weights::{ConvW, transpose_conv};

/// 反向后端要求 `cout % 4 == 0`：输出层按 4 通道补齐，只用通道 0。
const COUT_ALIGN: u32 = 4;
const NN: usize = 9; // 原始 3×3

struct Dev {
    wf: Buffer,
    wt: Buffer,
    ws: Buffer,
    bias: Buffer,
    wf_t: Buffer,
}

/// 一层卷积的 CPU 侧参数 + 设备镜像。
pub struct GConv {
    pub cin: u32,
    pub cout: u32,
    /// 补零后的核尺寸（`2·dil+1`）。
    pub kk: u32,
    pub dil: u32,
    pub ternary: bool,
    pub relu: bool,
    /// CPU 布局 `[((co·cin)+ci)·9 + (dy·3+dx)]` 的 latent 权重。
    pub w: Vec<f32>,
    pub b: Vec<f32>,
    pub scale: Vec<f32>,
    /// 三值量化结果（`ternary=false` 时不用）。
    pub q: Vec<i32>,
    gcout: u32,
    mw: Vec<f32>,
    vw: Vec<f32>,
    mb: Vec<f32>,
    vb: Vec<f32>,
    t: u32,
    dev: Option<Dev>,
}

fn quant(w: f32) -> i32 {
    w.round().clamp(-1.0, 1.0) as i32
}

impl GConv {
    /// 从 CPU 层参数建（`c.dil` 决定补零核尺寸）。
    pub fn from_cpu(c: &Conv, relu: bool) -> Self {
        let kk = (2 * c.dil + 1) as u32;
        let q = c.w.iter().map(|v| quant(*v)).collect();
        Self {
            cin: c.cin as u32,
            cout: c.cout as u32,
            kk,
            dil: c.dil as u32,
            ternary: c.ternary,
            relu,
            w: c.w.clone(),
            b: c.b.clone(),
            scale: c.scale.clone(),
            q,
            gcout: align4(c.cout as u32),
            mw: vec![0.0; c.w.len()],
            vw: vec![0.0; c.w.len()],
            mb: vec![0.0; c.b.len()],
            vb: vec![0.0; c.b.len()],
            t: 0,
            dev: None,
        }
    }

    fn spec(&self) -> ConvW {
        ConvW {
            wf: Vec::new(),
            wt: Vec::new(),
            ws: Vec::new(),
            bias: Vec::new(),
            cin: self.cin,
            cout: self.gcout,
            kk: self.kk,
        }
    }

    fn nw(&self) -> usize {
        (self.kk * self.kk * self.cin * self.gcout) as usize
    }

    /// 把参数写成内核要的布局并推到设备。
    pub fn upload(&mut self, gpu: &Gpu) {
        let (cin, cout, gcout) = (self.cin as usize, self.cout as usize, self.gcout as usize);
        let kk = self.kk as usize;
        let dil = self.dil as usize;
        let mut wf = vec![0.0f32; self.nw()];
        let mut wt = vec![0i32; self.nw()];
        for co in 0..cout {
            let s = self.scale[co];
            for ci in 0..cin {
                for dy in 0..3 {
                    for dx in 0..3 {
                        let wi = (co * cin + ci) * NN + dy * 3 + dx;
                        let p = if self.ternary {
                            self.q[wi] as f32
                        } else {
                            self.w[wi]
                        };
                        let gi = (((dy * dil) * kk + dx * dil) * cin + ci) * gcout + co;
                        wf[gi] = p * s;
                        wt[gi] = if self.ternary { self.q[wi] } else { 0 };
                    }
                }
            }
        }
        let mut ws = vec![1.0f32; gcout];
        let mut bias = vec![0.0f32; gcout];
        for co in 0..cout {
            ws[co] = self.scale[co];
            bias[co] = self.scale[co] * self.b[co];
        }
        let wf_t = transpose_conv(&wf, cin, gcout, kk);
        match &mut self.dev {
            Some(d) => {
                gpu.write(&d.wf, &wf);
                gpu.write_i32(&d.wt, &wt);
                gpu.write(&d.ws, &ws);
                gpu.write(&d.bias, &bias);
                gpu.write(&d.wf_t, &wf_t);
            }
            None => {
                self.dev = Some(Dev {
                    wf: gpu.buf(&wf, "dg-wf"),
                    wt: gpu.ibuf(&wt, "dg-wt"),
                    ws: gpu.buf(&ws, "dg-ws"),
                    bias: gpu.buf(&bias, "dg-b"),
                    wf_t: gpu.buf(&wf_t, "dg-wft"),
                });
            }
        }
    }

    /// 前向（`x` 是 HWC 输入缓冲，`size²` 像素）。
    pub fn forward(&self, gpu: &Gpu, x: &Buffer, size: u32) -> Buffer {
        let d = self.dev.as_ref().expect("层未上传");
        let mode = u32::from(self.ternary); // 1 = 三值 (wt·ws)，0 = f32 (wf)
        gpu.conv(
            x,
            size,
            size,
            &self.spec(),
            mode,
            u32::from(self.relu),
            &d.wf,
            &d.wt,
            &d.ws,
            &d.bias,
        )
    }

    /// 反向：只发 dispatch，不读回。返回 `(dw, db, dx)`，梯度是对**有效权重 E** 的。
    pub fn grads(
        &self,
        gpu: &Gpu,
        x: &Buffer,
        dy: &Buffer,
        size: u32,
        need_dx: bool,
    ) -> (Buffer, Buffer, Option<Buffer>) {
        let spec = self.spec();
        let dw = gpu.bwd_conv_dw_split(size, size, &spec, 0, x, dy);
        let db = gpu.bwd_conv_db_split(size, size, &spec, 0, dy);
        let dx = need_dx.then(|| {
            let d = self.dev.as_ref().expect("层未上传");
            gpu.bwd_conv_dx(size, size, &spec, 0, dy, &d.wf, &d.wf_t)
        });
        (dw, db, dx)
    }

    /// 把设备梯度读回并**按 CPU 口径还原**：`dw_cpu = scale·dE`、`db_cpu = scale·db`、
    /// `dscale = Σ_ci,k p·dE + b·db`。
    fn pull(&self, dwv: &[f32], dbv: &[f32]) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
        let (cin, cout, gcout) = (self.cin as usize, self.cout as usize, self.gcout as usize);
        let kk = self.kk as usize;
        let dil = self.dil as usize;
        let mut dw_cpu = vec![0.0f32; cout * cin * NN];
        let mut db_cpu = vec![0.0f32; cout];
        let mut dsc = vec![0.0f32; cout];
        for co in 0..cout {
            let s = self.scale[co];
            db_cpu[co] = s * dbv[co];
            dsc[co] = self.b[co] * dbv[co];
            for ci in 0..cin {
                for dy in 0..3 {
                    for dx in 0..3 {
                        let gi = (((dy * dil) * kk + dx * dil) * cin + ci) * gcout + co;
                        let wi = (co * cin + ci) * NN + dy * 3 + dx;
                        let p = if self.ternary {
                            self.q[wi] as f32
                        } else {
                            self.w[wi]
                        };
                        dw_cpu[wi] = s * dwv[gi];
                        dsc[co] += p * dwv[gi];
                    }
                }
            }
        }
        (dw_cpu, db_cpu, dsc)
    }

    /// 一步更新（与 CPU `Conv::apply` 同式）：Adam(w,b) + SGD(scale)，随后重新量化。
    #[allow(clippy::needless_range_loop)] // 三路并行索引（w/m/v），迭代器写法反而绕
    fn apply(&mut self, dw: &[f32], db: &[f32], dsc: &[f32], lr: f32) {
        self.t += 1;
        let (b1, b2, eps) = (0.9f32, 0.999f32, 1e-8f32);
        let bc1 = 1.0 - b1.powi(self.t as i32);
        let bc2 = 1.0 - b2.powi(self.t as i32);
        for i in 0..self.w.len() {
            self.mw[i] = b1 * self.mw[i] + (1.0 - b1) * dw[i];
            self.vw[i] = b2 * self.vw[i] + (1.0 - b2) * dw[i] * dw[i];
            self.w[i] -= lr * (self.mw[i] / bc1) / ((self.vw[i] / bc2).sqrt() + eps);
        }
        for i in 0..self.b.len() {
            self.mb[i] = b1 * self.mb[i] + (1.0 - b1) * db[i];
            self.vb[i] = b2 * self.vb[i] + (1.0 - b2) * db[i] * db[i];
            self.b[i] -= lr * (self.mb[i] / bc1) / ((self.vb[i] / bc2).sqrt() + eps);
            self.scale[i] = (self.scale[i] - lr * dsc[i]).clamp(1e-3, 10.0);
        }
        if self.ternary {
            // CPU 的量化只取整 latent（scale 在外层）：q = round(clamp(w))
            for (q, w) in self.q.iter_mut().zip(&self.w) {
                *q = quant(*w);
            }
        }
    }
}

fn align4(v: u32) -> u32 {
    v.div_ceil(COUT_ALIGN) * COUT_ALIGN
}

/// 整个深度头的 GPU 镜像。
pub struct DepthGpu {
    pub c1: GConv,
    pub c2: GConv,
    pub c3: GConv,
    pub out_b: f32,
    pub med: f32,
    pub iqr: f32,
    pub size: u32,
    /// 分项耗时（秒）：打包+前向+读回 / 造 dy / 反向 dispatch / 读回+更新+上传。
    pub prof: [f64; 4],
}

impl DepthGpu {
    /// 从 CPU 网建（用于同一初始化的对拍）。
    pub fn from_depthnet(net: &DepthNet, size: u32) -> Self {
        Self {
            c1: GConv::from_cpu(&net.c1, true),
            c2: GConv::from_cpu(&net.c2, true),
            c3: GConv::from_cpu(&net.c3, false),
            out_b: net.out_b,
            med: net.med,
            iqr: net.iqr,
            size,
            prof: [0.0; 4],
        }
    }

    /// 导回 CPU 网（存 DCN3 / 走现有 `--eval-only` 评测）。
    pub fn to_depthnet(&self) -> DepthNet {
        let mut net = DepthNet::with_inputs(
            self.c1.cin as usize,
            self.c1.cout as usize,
            1,
            self.c1.ternary,
        );
        for (dst, src) in [
            (&mut net.c1, &self.c1),
            (&mut net.c2, &self.c2),
            (&mut net.c3, &self.c3),
        ] {
            dst.w = src.w.clone();
            dst.b = src.b.clone();
            dst.scale = src.scale.clone();
            dst.ternary = src.ternary;
            dst.dil = src.dil as usize;
        }
        net.out_b = self.out_b;
        net.med = self.med;
        net.iqr = self.iqr;
        net
    }

    pub fn upload(&mut self, gpu: &Gpu) {
        self.c1.upload(gpu);
        self.c2.upload(gpu);
        self.c3.upload(gpu);
    }

    fn pack(&self, x: &[Vec<f32>]) -> Vec<f32> {
        let hw = (self.size * self.size) as usize;
        let cin = self.c1.cin as usize;
        let mut v = vec![0.0f32; hw * cin];
        for (c, plane) in x.iter().enumerate().take(cin) {
            for (i, val) in plane.iter().enumerate() {
                v[i * cin + c] = *val;
            }
        }
        v
    }

    /// 前向：返回 HWC 的 `size²` 像素（已加 `out_b`）。
    pub fn predict(&self, gpu: &Gpu, x: &[Vec<f32>]) -> Vec<f32> {
        let hw = (self.size * self.size) as usize;
        let xin = self.pack(x);
        let xb = gpu.buf(&xin, "dg-x");
        gpu.begin_batch();
        let a1 = self.c1.forward(gpu, &xb, self.size);
        let a2 = self.c2.forward(gpu, &a1, self.size);
        let o3 = self.c3.forward(gpu, &a2, self.size);
        gpu.end_batch();
        let out = gpu.read(&o3, hw * self.c3.gcout as usize);
        let gco = self.c3.gcout as usize;
        (0..hw).map(|i| out[i * gco] + self.out_b).collect()
    }

    /// 一步训练：损失 = 有效像素上的均方误差（与 CPU `train_step` 同口径）。
    pub fn train_step(
        &mut self,
        gpu: &Gpu,
        x: &[Vec<f32>],
        target: &[f32],
        mask: &[bool],
        lr: f32,
    ) -> f32 {
        let size = self.size;
        let hw = (size * size) as usize;
        let gco = self.c3.gcout as usize;
        let cin1 = self.c2.cout as usize; // c1 输出 / c2 输入通道
        let t0 = std::time::Instant::now();
        let xin = self.pack(x);
        let xb = gpu.buf(&xin, "dg-x");

        // 前向
        gpu.begin_batch();
        let a1 = self.c1.forward(gpu, &xb, size);
        let a2 = self.c2.forward(gpu, &a1, size);
        let o3 = self.c3.forward(gpu, &a2, size);
        gpu.end_batch();
        let out = gpu.read(&o3, hw * gco);
        self.prof[0] += t0.elapsed().as_secs_f64();

        // 损失与 dL/dout（只填通道 0）
        let t1 = std::time::Instant::now();
        let mut dy = vec![0.0f32; hw * gco];
        let mut loss = 0.0f32;
        let mut valid = 0usize;
        let mut gb = 0.0f32;
        for i in 0..hw {
            if !mask[i] {
                continue;
            }
            let d = out[i * gco] + self.out_b - target[i];
            loss += d * d;
            valid += 1;
            gb += 2.0 * d;
        }
        let inv = 1.0 / valid.max(1) as f32;
        for i in 0..hw {
            if mask[i] {
                dy[i * gco] = 2.0 * (out[i * gco] + self.out_b - target[i]) * inv;
            }
        }
        self.out_b -= lr * gb * inv;
        let dy3 = gpu.buf(&dy, "dg-dy3");
        self.prof[1] += t1.elapsed().as_secs_f64();

        // 反向
        let t2 = std::time::Instant::now();
        gpu.begin_batch();
        let (dw3, db3, dx3) = self.c3.grads(gpu, &a2, &dy3, size, true);
        let dy2 = gpu.mask_dy(&a2, dx3.as_ref().unwrap(), hw * cin1, true);
        let (dw2, db2, dx2) = self.c2.grads(gpu, &a1, &dy2, size, true);
        let dy1 = gpu.mask_dy(&a1, dx2.as_ref().unwrap(), hw * cin1, true);
        let (dw1, db1, _) = self.c1.grads(gpu, &xb, &dy1, size, false);
        gpu.end_batch();
        self.prof[2] += t2.elapsed().as_secs_f64();

        let t3 = std::time::Instant::now();
        let r = gpu.read_many(&[
            (&dw1, self.c1.nw()),
            (&db1, self.c1.gcout as usize),
            (&dw2, self.c2.nw()),
            (&db2, self.c2.gcout as usize),
            (&dw3, self.c3.nw()),
            (&db3, self.c3.gcout as usize),
        ]);
        let (g1, gb1, s1) = self.c1.pull(&r[0], &r[1]);
        let (g2, gb2, s2) = self.c2.pull(&r[2], &r[3]);
        let (g3, gb3, s3) = self.c3.pull(&r[4], &r[5]);
        self.c1.apply(&g1, &gb1, &s1, lr);
        self.c2.apply(&g2, &gb2, &s2, lr);
        self.c3.apply(&g3, &gb3, &s3, lr);
        self.c1.upload(gpu);
        self.c2.upload(gpu);
        self.c3.upload(gpu);
        self.prof[3] += t3.elapsed().as_secs_f64();
        loss * inv
    }
}
