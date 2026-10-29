//! 与设备无关的模型表示 + 后端抽象。
//!
//! 目标：同一份三值权重，能在 wgpu（Vulkan/Metal/D3D12）、CPU 参考实现、以及后续的
//! NEON/LUT/NPU 后端上跑出**可对拍**的结果。设计见 `docs/MULTI-CHIP.md`。
//!
//! 这里的 `TriModel` 只装「推理要用的东西」：有效权重 `E = q·scale`（f32）、
//! 折叠后的 bias、以及 affine 的 γ/β。训练态（latent/Adam/RNG）留在 `net.rs`。

use crate::weights::{ConvW, Tensors, conv_w, convt_w};

/// 一层推理参数。
#[derive(Clone)]
pub struct LayerW {
    /// 有效权重 `E = q·scale`，布局 `[(ky·kk+kx)·cin + ic]·cout + oc`（转置卷积为 `[(kh·2+kw)·cin+ci]·cout+co`）。
    pub e: Vec<f32>,
    pub bias: Vec<f32>,
    pub gamma: Vec<f32>,
    pub beta: Vec<f32>,
    pub cin: u32,
    pub cout: u32,
    pub kk: u32,
    pub relu: bool,
    pub convt: bool,
}

impl LayerW {
    fn from_conv_w(cw: &ConvW, relu: bool) -> Self {
        let slots = cw.cout as usize;
        let e: Vec<f32> = (0..cw.wf.len())
            .map(|wi| cw.wt[wi] as f32 * cw.ws[wi % slots])
            .collect();
        Self {
            e,
            bias: cw.bias.clone(),
            gamma: vec![1.0; slots],
            beta: vec![0.0; slots],
            cin: cw.cin,
            cout: cw.cout,
            kk: cw.kk,
            relu,
            convt: false,
        }
    }
}

/// 推理模型（层序与 `net.rs` 的常量一致）。
pub struct TriModel {
    pub layers: Vec<LayerW>,
}

/// 层序号（与训练侧一致）。
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

impl TriModel {
    /// 从推理 ckpt 建模型（BN 折叠 → 三值化 → `E`）。
    pub fn from_tensors(t: &Tensors) -> Result<Self, String> {
        let mut layers = vec![
            LayerW::from_conv_w(&conv_w(t, "e1.0.weight", Some("e1.1"), true)?, true),
            LayerW::from_conv_w(&conv_w(t, "e1.3.weight", Some("e1.4"), true)?, true),
            LayerW::from_conv_w(&conv_w(t, "e2.0.weight", Some("e2.1"), true)?, true),
            LayerW::from_conv_w(&conv_w(t, "e2.3.weight", Some("e2.4"), true)?, true),
            LayerW::from_conv_w(&conv_w(t, "e3.0.weight", Some("e3.1"), true)?, true),
            LayerW::from_conv_w(&conv_w(t, "e3.3.weight", Some("e3.4"), true)?, true),
            LayerW::from_conv_w(&convt_w(t, "u3.weight")?, false),
            LayerW::from_conv_w(&conv_w(t, "d2.0.weight", Some("d2.1"), true)?, true),
            LayerW::from_conv_w(&conv_w(t, "d2.3.weight", Some("d2.4"), true)?, true),
            LayerW::from_conv_w(&convt_w(t, "u2.weight")?, false),
            LayerW::from_conv_w(&conv_w(t, "d1.0.weight", Some("d1.1"), true)?, true),
            LayerW::from_conv_w(&conv_w(t, "d1.3.weight", Some("d1.4"), true)?, true),
            LayerW::from_conv_w(&conv_w(t, "head.weight", None, false)?, false),
        ];
        layers[U3].convt = true;
        layers[U2].convt = true;
        Ok(Self { layers })
    }
}

/// 后端能力（调度层用来选实现）。
#[derive(Clone, Debug)]
pub struct Caps {
    pub name: &'static str,
    pub max_buffer: u64,
    pub int8: bool,
    pub lut: bool,
}

/// 推理后端。**所有后端必须满足同一组门**（见 `docs/MULTI-CHIP.md` §5）：
/// 同一份 `E` 下，与参考实现比 `max|Δlogit|` 与平面 argmax 一致率。
pub trait Backend {
    fn caps(&self) -> Caps;
    /// 跑一帧：NHWC 的 `res × res × 3` 输入，返回 NHWC 的 `res × res × 16` logits。
    fn forward(&mut self, x: &[f32], res: u32) -> Result<Vec<f32>, String>;
}

// ---------------------------------------------------------------- CPU 参考后端（f32）

/// 纯标量 f32 参考实现：**没有 GPU 也必须能跑**，是所有其它后端的对拍基准。
pub struct RefCpu {
    model: TriModel,
}

impl RefCpu {
    pub fn new(model: TriModel) -> Self {
        Self { model }
    }

    fn conv(&self, x: &[f32], h: u32, w: u32, l: &LayerW) -> Vec<f32> {
        let (cin, cout, kk, pad) = (
            l.cin as usize,
            l.cout as usize,
            l.kk as usize,
            (l.kk / 2) as isize,
        );
        let mut y = vec![0f32; (h as usize) * (w as usize) * cout];
        for oy in 0..h as usize {
            for ox in 0..w as usize {
                for oc in 0..cout {
                    let mut acc = l.bias[oc];
                    for ky in 0..kk {
                        let iy = oy as isize + ky as isize - pad;
                        if iy < 0 || iy >= h as isize {
                            continue;
                        }
                        for kx in 0..kk {
                            let ix = ox as isize + kx as isize - pad;
                            if ix < 0 || ix >= w as isize {
                                continue;
                            }
                            let xb = ((iy as usize) * w as usize + ix as usize) * cin;
                            let wb = ((ky * kk + kx) * cin) * cout;
                            for ic in 0..cin {
                                acc += x[xb + ic] * l.e[wb + ic * cout + oc];
                            }
                        }
                    }
                    y[(oy * w as usize + ox) * cout + oc] = acc;
                }
            }
        }
        y
    }

    fn convt(&self, x: &[f32], h: u32, w: u32, l: &LayerW) -> Vec<f32> {
        let (cin, cout) = (l.cin as usize, l.cout as usize);
        let (oh, ow) = ((h * 2) as usize, (w * 2) as usize);
        let mut y = vec![0f32; oh * ow * cout];
        for iy in 0..h as usize {
            for ix in 0..w as usize {
                for kh in 0..2usize {
                    for kw in 0..2usize {
                        let ob = ((iy * 2 + kh) * ow + ix * 2 + kw) * cout;
                        for co in 0..cout {
                            let mut acc = l.bias[co];
                            for ci in 0..cin {
                                acc += x[(iy * w as usize + ix) * cin + ci]
                                    * l.e[((kh * 2 + kw) * cin + ci) * cout + co];
                            }
                            y[ob + co] = acc;
                        }
                    }
                }
            }
        }
        y
    }

    fn affine(&self, v: &[f32], l: &LayerW) -> Vec<f32> {
        let cout = l.cout as usize;
        v.iter()
            .enumerate()
            .map(|(i, &x)| {
                let oc = i % cout;
                let t = l.gamma[oc] * x + l.beta[oc];
                if l.relu { t.max(0.0) } else { t }
            })
            .collect()
    }

    fn block(&self, x: &[f32], h: u32, w: u32, i: usize) -> Vec<f32> {
        let v = self.conv(x, h, w, &self.model.layers[i]);
        self.affine(&v, &self.model.layers[i])
    }

    fn pool(&self, x: &[f32], h: u32, w: u32, c: u32) -> Vec<f32> {
        let (oh, ow, cc) = ((h / 2) as usize, (w / 2) as usize, c as usize);
        let mut y = vec![0f32; oh * ow * cc];
        for oy in 0..oh {
            for ox in 0..ow {
                for ch in 0..cc {
                    let mut m = f32::NEG_INFINITY;
                    for dy in 0..2usize {
                        for dx in 0..2usize {
                            let v = x[((oy * 2 + dy) * w as usize + ox * 2 + dx) * cc + ch];
                            m = m.max(v);
                        }
                    }
                    y[(oy * ow + ox) * cc + ch] = m;
                }
            }
        }
        y
    }

    fn cat(a: &[f32], b: &[f32], hw: usize, ca: usize, cb: usize) -> Vec<f32> {
        let mut y = vec![0f32; hw * (ca + cb)];
        for p in 0..hw {
            y[p * (ca + cb)..p * (ca + cb) + ca].copy_from_slice(&a[p * ca..(p + 1) * ca]);
            y[p * (ca + cb) + ca..(p + 1) * (ca + cb)].copy_from_slice(&b[p * cb..(p + 1) * cb]);
        }
        y
    }
}

impl Backend for RefCpu {
    fn caps(&self) -> Caps {
        Caps {
            name: "refcpu",
            max_buffer: u64::MAX,
            int8: false,
            lut: false,
        }
    }

    fn forward(&mut self, x: &[f32], res: u32) -> Result<Vec<f32>, String> {
        let h2 = res / 2;
        let h4 = res / 4;
        let a = self.block(x, res, res, E1A);
        let a = self.block(&a, res, res, E1B);
        let p1 = self.pool(&a, res, res, 32);
        let b = self.block(&p1, h2, h2, E2A);
        let b = self.block(&b, h2, h2, E2B);
        let p2 = self.pool(&b, h2, h2, 64);
        let c = self.block(&p2, h4, h4, E3A);
        let c = self.block(&c, h4, h4, E3B);
        let u3 = self.convt(&c, h4, h4, &self.model.layers[U3]);
        let cat1 = Self::cat(&u3, &b, (h2 * h2) as usize, 64, 64);
        let d2 = self.block(&cat1, h2, h2, D2A);
        let d2 = self.block(&d2, h2, h2, D2B);
        let u2 = self.convt(&d2, h2, h2, &self.model.layers[U2]);
        let cat2 = Self::cat(&u2, &a, (res * res) as usize, 32, 32);
        let d1 = self.block(&cat2, res, res, D1A);
        let d1 = self.block(&d1, res, res, D1B);
        Ok(self.block(&d1, res, res, HEAD))
    }
}

/// 两个**同布局（NHWC）**输出之间的平面 argmax 一致率与最大逐元素差。
///
/// 注意：`compare` 的 `want` 是 Python 的 CHW 布局；两个 Rust 后端之间必须用这个函数，
/// 否则会拿 NHWC 当 CHW 比（实测会把一致率误判成 0.34）。
pub fn compare_nhwc(a: &[f32], b: &[f32], res: u32) -> (f64, f32) {
    let n = (res * res) as usize;
    assert_eq!(a.len(), n * 16);
    assert_eq!(b.len(), n * 16);
    let (mut agree, mut maxd) = (0usize, 0f32);
    for i in 0..5 {
        for p in 0..n {
            let (mut ra, mut rb) = (0usize, 0usize);
            let (mut va, mut vb) = (f32::MIN, f32::MIN);
            for c in 0..3 {
                let x = a[p * 16 + i * 3 + c];
                let y = b[p * 16 + i * 3 + c];
                maxd = maxd.max((x - y).abs());
                if x > va {
                    va = x;
                    ra = c;
                }
                if y > vb {
                    vb = y;
                    rb = c;
                }
            }
            agree += usize::from(ra == rb);
        }
    }
    (agree as f64 / (5 * n) as f64, maxd)
}

/// 平面 argmax 一致率（与 `parts-parity` 同口径）与最大逐元素差。
pub fn compare(out: &[f32], want: &[f32], res: u32) -> (f64, f32) {
    let n = (res * res) as usize;
    assert_eq!(out.len(), n * 16);
    assert_eq!(want.len(), n * 16);
    let mut agree = 0usize;
    let mut maxd = 0f32;
    for i in 0..5 {
        for p in 0..n {
            let (mut ra, mut rb) = (0usize, 0usize);
            let (mut va, mut vb) = (f32::MIN, f32::MIN);
            for c in 0..3 {
                let a = out[p * 16 + i * 3 + c];
                let b = want[(i * 3 + c) * n + p];
                maxd = maxd.max((a - b).abs());
                if a > va {
                    va = a;
                    ra = c;
                }
                if b > vb {
                    vb = b;
                    rb = c;
                }
            }
            agree += usize::from(ra == rb);
        }
    }
    (agree as f64 / (5 * n) as f64, maxd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refcpu_is_deterministic_and_shaped() {
        // 用随机小模型验证形状与确定性（不依赖权重文件）
        let layers = vec![LayerW {
            e: vec![0.5; 3 * 3 * 3 * 4],
            bias: vec![0.0; 4],
            gamma: vec![1.0; 4],
            beta: vec![0.0; 4],
            cin: 3,
            cout: 4,
            kk: 3,
            relu: true,
            convt: false,
        }];
        let cpu = RefCpu {
            model: TriModel { layers },
        };
        let x = vec![0.1f32; 8 * 8 * 3];
        let y1 = cpu.conv(&x, 8, 8, &cpu.model.layers[0]);
        let y2 = cpu.conv(&x, 8, 8, &cpu.model.layers[0]);
        assert_eq!(y1, y2);
        assert_eq!(y1.len(), 8 * 8 * 4);
    }
}
