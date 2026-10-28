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

    let up = |cw: &crate::weights::ConvW, tag: &str| {
        (
            gpu.buf(&cw.wf, &format!("{tag}.wf")),
            gpu.ibuf(&cw.wt, &format!("{tag}.wt")),
            gpu.buf(&cw.ws, &format!("{tag}.ws")),
            gpu.buf(&cw.bias, &format!("{tag}.b")),
        )
    };

    // e1: 3→32, 32→32 @ res
    let w = conv_w(t, "e1.0.weight", Some("e1.1"), true).expect("e1.0");
    let (a1, b1, c1, d1) = up(&w, "e1a");
    let mut cur = gpu.conv(x, res, res, &w, mode, 1, &a1, &b1, &c1, &d1);
    let w = conv_w(t, "e1.3.weight", Some("e1.4"), true).expect("e1.3");
    let (a1, b1, c1, d1) = up(&w, "e1b");
    let a = gpu.conv(&cur, res, res, &w, mode, 1, &a1, &b1, &c1, &d1);
    snap("a", &a, (res * res * 32) as usize);

    // e2 @ res/2
    let p = gpu.pool(&a, res, res, 32);
    let h2 = res / 2;
    let w = conv_w(t, "e2.0.weight", Some("e2.1"), true).expect("e2.0");
    let (a1, b1, c1, d1) = up(&w, "e2a");
    cur = gpu.conv(&p, h2, h2, &w, mode, 1, &a1, &b1, &c1, &d1);
    let w = conv_w(t, "e2.3.weight", Some("e2.4"), true).expect("e2.3");
    let (a1, b1, c1, d1) = up(&w, "e2b");
    let b = gpu.conv(&cur, h2, h2, &w, mode, 1, &a1, &b1, &c1, &d1);
    snap("b", &b, (h2 * h2 * 64) as usize);

    // e3 @ res/4
    let p = gpu.pool(&b, h2, h2, 64);
    let h4 = res / 4;
    let w = conv_w(t, "e3.0.weight", Some("e3.1"), true).expect("e3.0");
    let (a1, b1, c1, d1) = up(&w, "e3a");
    cur = gpu.conv(&p, h4, h4, &w, mode, 1, &a1, &b1, &c1, &d1);
    let w = conv_w(t, "e3.3.weight", Some("e3.4"), true).expect("e3.3");
    let (a1, b1, c1, d1) = up(&w, "e3b");
    let c = gpu.conv(&cur, h4, h4, &w, mode, 1, &a1, &b1, &c1, &d1);
    snap("c", &c, (h4 * h4 * 128) as usize);

    // u3: 128→64 @ res/2
    let w = convt_w(t, "u3.weight").expect("u3");
    let (a1, b1, c1, d1) = up(&w, "u3");
    let u3 = gpu.convt(&c, h4, h4, &w, mode, &a1, &b1, &c1, &d1);
    snap("u3", &u3, (h2 * h2 * 64) as usize);

    // d2: cat(u3,b) 128→64, 64→64
    let cat = gpu.cat(&u3, &b, h2 * h2, 64, 64);
    let w = conv_w(t, "d2.0.weight", Some("d2.1"), true).expect("d2.0");
    let (a1, b1, c1, d1) = up(&w, "d2a");
    cur = gpu.conv(&cat, h2, h2, &w, mode, 1, &a1, &b1, &c1, &d1);
    let w = conv_w(t, "d2.3.weight", Some("d2.4"), true).expect("d2.3");
    let (a1, b1, c1, d1) = up(&w, "d2b");
    let y = gpu.conv(&cur, h2, h2, &w, mode, 1, &a1, &b1, &c1, &d1);
    snap("y", &y, (h2 * h2 * 64) as usize);

    // u2: 64→32 @ res
    let w = convt_w(t, "u2.weight").expect("u2");
    let (a1, b1, c1, d1) = up(&w, "u2");
    let u2 = gpu.convt(&y, h2, h2, &w, mode, &a1, &b1, &c1, &d1);
    snap("u2", &u2, (res * res * 32) as usize);

    // d1: cat(u2,a) 64→32, 32→32
    let cat = gpu.cat(&u2, &a, res * res, 32, 32);
    let w = conv_w(t, "d1.0.weight", Some("d1.1"), true).expect("d1.0");
    let (a1, b1, c1, d1) = up(&w, "d1a");
    cur = gpu.conv(&cat, res, res, &w, mode, 1, &a1, &b1, &c1, &d1);
    let w = conv_w(t, "d1.3.weight", Some("d1.4"), true).expect("d1.3");
    let (a1, b1, c1, d1) = up(&w, "d1b");
    let z = gpu.conv(&cur, res, res, &w, mode, 1, &a1, &b1, &c1, &d1);
    snap("z", &z, (res * res * 32) as usize);

    // head: 1x1 32→16，无 ReLU
    let w = conv_w(t, "head.weight", None, false).expect("head");
    let (a1, b1, c1, d1) = up(&w, "head");
    let out = gpu.conv(&z, res, res, &w, mode, 0, &a1, &b1, &c1, &d1);
    gpu.read(&out, (res * res * 16) as usize)
}
