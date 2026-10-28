//! UNet 前向（NHWC），f32 与三值双模。
//!
//! 两条路径共用同一份折叠后的 bias：`mode = 0` 走稠密 f32 权重，
//! `mode = 1` 走每通道三值 `{-1,0,+1} × scale`。两者的差就是量化损失本身。

use std::path::Path;

use crate::gpu::Gpu;
use crate::weights::{Tensors, conv_w, convt_w};

/// 跑一帧 `res × res × 3` 输入，返回 NHWC 的 `res × res × 16` logits。
///
/// `dump` 给了目录时，把各层中间激活写成 `mid_<名字>.bin`（与 Python 侧逐层对拍用）。
pub fn forward(
    gpu: &Gpu,
    t: &Tensors,
    x: &wgpu::Buffer,
    res: u32,
    mode: u32,
    dump: Option<&Path>,
) -> Vec<f32> {
    let snap = |name: &str, buf: &wgpu::Buffer, n: usize| {
        if let Some(d) = dump {
            let v = gpu.read(buf, n);
            let mut bytes = Vec::with_capacity(v.len() * 4);
            for f in &v {
                bytes.extend_from_slice(&f.to_le_bytes());
            }
            std::fs::write(d.join(format!("mid_{name}.bin")), bytes).ok();
            println!("  转储 mid_{name} ({n} 元素)");
        }
    };

    // 前向统一走「有效权重 E」：mode=0 取折叠后的稠密权重，mode=1 取 q·scale。
    // cout 是 4 的倍数时用 vec4 内核（与训练同一条），否则退回标量路径。
    let up = |cw: &crate::weights::ConvW, tag: &str| {
        let eff: Vec<f32> = (0..cw.wf.len())
            .map(|wi| {
                if mode == 0 {
                    cw.wf[wi]
                } else {
                    cw.wt[wi] as f32 * cw.ws[wi % cw.cout as usize]
                }
            })
            .collect();
        (
            gpu.buf(&eff, &format!("{tag}.e")),
            gpu.ibuf(&cw.wt, &format!("{tag}.wt")),
            gpu.buf(&cw.ws, &format!("{tag}.ws")),
            gpu.buf(&cw.bias, &format!("{tag}.b")),
        )
    };

    // e1: 3→32, 32→32 @ res
    let w = conv_w(t, "e1.0.weight", Some("e1.1"), true).expect("e1.0");
    let (a1, _b1, _c1, d1) = up(&w, "e1a");
    let mut cur = conv_layer(gpu, x, res, res, &w, 1, &a1, &d1);
    let w = conv_w(t, "e1.3.weight", Some("e1.4"), true).expect("e1.3");
    let (a1, _b1, _c1, d1) = up(&w, "e1b");
    let a = conv_layer(gpu, &cur, res, res, &w, 1, &a1, &d1);
    snap("a", &a, (res * res * 32) as usize);

    // e2 @ res/2
    let p = gpu.pool(&a, res, res, 32);
    let h2 = res / 2;
    let w = conv_w(t, "e2.0.weight", Some("e2.1"), true).expect("e2.0");
    let (a1, _b1, _c1, d1) = up(&w, "e2a");
    cur = conv_layer(gpu, &p, h2, h2, &w, 1, &a1, &d1);
    let w = conv_w(t, "e2.3.weight", Some("e2.4"), true).expect("e2.3");
    let (a1, _b1, _c1, d1) = up(&w, "e2b");
    let b = conv_layer(gpu, &cur, h2, h2, &w, 1, &a1, &d1);
    snap("b", &b, (h2 * h2 * 64) as usize);

    // e3 @ res/4
    let p = gpu.pool(&b, h2, h2, 64);
    let h4 = res / 4;
    let w = conv_w(t, "e3.0.weight", Some("e3.1"), true).expect("e3.0");
    let (a1, _b1, _c1, d1) = up(&w, "e3a");
    cur = conv_layer(gpu, &p, h4, h4, &w, 1, &a1, &d1);
    let w = conv_w(t, "e3.3.weight", Some("e3.4"), true).expect("e3.3");
    let (a1, _b1, _c1, d1) = up(&w, "e3b");
    let c = conv_layer(gpu, &cur, h4, h4, &w, 1, &a1, &d1);
    snap("c", &c, (h4 * h4 * 128) as usize);

    // u3: 128→64 @ res/2
    let w = convt_w(t, "u3.weight").expect("u3");
    let (a1, b1, c1, d1) = up(&w, "u3");
    let u3 = gpu.convt(&c, h4, h4, &w, mode, &a1, &b1, &c1, &d1);
    snap("u3", &u3, (h2 * h2 * 64) as usize);

    // d2: cat(u3,b) 128→64, 64→64
    let cat = gpu.cat(&u3, &b, h2 * h2, 64, 64);
    let w = conv_w(t, "d2.0.weight", Some("d2.1"), true).expect("d2.0");
    let (a1, _b1, _c1, d1) = up(&w, "d2a");
    cur = conv_layer(gpu, &cat, h2, h2, &w, 1, &a1, &d1);
    let w = conv_w(t, "d2.3.weight", Some("d2.4"), true).expect("d2.3");
    let (a1, _b1, _c1, d1) = up(&w, "d2b");
    let y = conv_layer(gpu, &cur, h2, h2, &w, 1, &a1, &d1);
    snap("y", &y, (h2 * h2 * 64) as usize);

    // u2: 64→32 @ res
    let w = convt_w(t, "u2.weight").expect("u2");
    let (a1, b1, c1, d1) = up(&w, "u2");
    let u2 = gpu.convt(&y, h2, h2, &w, mode, &a1, &b1, &c1, &d1);
    snap("u2", &u2, (res * res * 32) as usize);

    // d1: cat(u2,a) 64→32, 32→32
    let cat = gpu.cat(&u2, &a, res * res, 32, 32);
    let w = conv_w(t, "d1.0.weight", Some("d1.1"), true).expect("d1.0");
    let (a1, _b1, _c1, d1) = up(&w, "d1a");
    cur = conv_layer(gpu, &cat, res, res, &w, 1, &a1, &d1);
    let w = conv_w(t, "d1.3.weight", Some("d1.4"), true).expect("d1.3");
    let (a1, _b1, _c1, d1) = up(&w, "d1b");
    let z = conv_layer(gpu, &cur, res, res, &w, 1, &a1, &d1);
    snap("z", &z, (res * res * 32) as usize);

    // head: 1x1 32→16，无 ReLU
    let w = conv_w(t, "head.weight", None, false).expect("head");
    let (a1, _b1, _c1, d1) = up(&w, "head");
    let out = conv_layer(gpu, &z, res, res, &w, 0, &a1, &d1);
    gpu.read(&out, (res * res * 16) as usize)
}

/// 一层卷积：`cout` 是 4 的倍数走 vec4 内核，否则退回标量（`eff` 已按 mode 折好）。
#[allow(clippy::too_many_arguments)]
fn conv_layer(
    gpu: &Gpu,
    x: &wgpu::Buffer,
    h: u32,
    w: u32,
    cw: &crate::weights::ConvW,
    relu: u32,
    eff: &wgpu::Buffer,
    bias: &wgpu::Buffer,
) -> wgpu::Buffer {
    if cw.cout.is_multiple_of(4) && std::env::var_os("TRIT_SCALAR").is_none() {
        gpu.conv_t(x, h, w, cw, relu == 1, eff, bias)
    } else {
        let ones = vec![1.0f32; cw.cout as usize];
        let ws = gpu.buf(&ones, "ones");
        gpu.conv(x, h, w, cw, 0, relu, eff, eff, &ws, bias)
    }
}
