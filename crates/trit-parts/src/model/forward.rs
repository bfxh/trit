//! `RefCpu` 的前向小块（拆出 `model.rs` 守模块行数；第 32 轮 32-1 修层下标越界）。

use super::RefCpu;

impl RefCpu {
    pub(super) fn block(&self, x: &[f32], h: u32, w: u32, i: usize) -> Vec<f32> {
        let Some(l) = self.model.layers.get(i) else {
            return Vec::new();
        }; // 32-1
        let v = self.conv(x, h, w, l);
        self.affine(&v, l)
    }

    pub(super) fn pool(&self, x: &[f32], h: u32, w: u32, c: u32) -> Vec<f32> {
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

    pub(super) fn cat(a: &[f32], b: &[f32], hw: usize, ca: usize, cb: usize) -> Vec<f32> {
        let mut y = vec![0f32; hw * (ca + cb)];
        for p in 0..hw {
            y[p * (ca + cb)..p * (ca + cb) + ca].copy_from_slice(&a[p * ca..(p + 1) * ca]);
            y[p * (ca + cb) + ca..(p + 1) * (ca + cb)].copy_from_slice(&b[p * cb..(p + 1) * cb]);
        }
        y
    }
}
