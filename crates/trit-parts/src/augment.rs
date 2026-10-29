//! 合成数据：在**现有帧**上做标签一致的随机扰动（不新增渲染，不引入 Python）。
//!
//! 允许的变换只有两类，且都不改变语义：
//! - **光度**：亮度 / 对比度 / 每通道增益 / 高斯噪声（只动 RGB，标签原样）；
//! - **平移**：整幅平移 (dx,dy)，RGB 边缘复制，标签同步平移，**移进来的边界像素 valid=0**
//!   （那里没有标注，标成"未知"而不是猜背景）。
//!
//! 🔴 明确禁止：左右/上下翻转。本任务的部件 id 与空间方位绑定（`arm_l_*` vs `arm_r_*`），
//! 实测翻转后留出 id 从 0.6913 掉到 0.4455（见 LEDGER A68）。

use crate::data::Sample;

pub struct Augment {
    state: u64,
    /// 平移幅度上限（像素）；0 表示不平移。
    pub shift: i32,
}

impl Augment {
    pub fn new(seed: u64, shift: i32) -> Self {
        Self {
            state: seed | 1,
            shift,
        }
    }

    fn next_u32(&mut self) -> u32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        (self.state >> 32) as u32
    }

    /// `[0,1)` 均匀。
    fn unit(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }

    /// 就地扰动一帧。
    pub fn apply(&mut self, res: u32, s: &mut Sample) {
        let r = res as usize;
        // ---- 光度 ----
        let bright = self.range(0.92, 1.08);
        let contrast = self.range(0.92, 1.08);
        let gain = [
            self.range(0.96, 1.04),
            self.range(0.96, 1.04),
            self.range(0.96, 1.04),
        ];
        let noise = self.range(0.0, 0.02);
        for p in 0..r * r {
            for (c, &g) in gain.iter().enumerate() {
                let i = p * 3 + c;
                let v = (s.rgb[i] * bright - 0.5) * contrast + 0.5;
                let v = v * g;
                let n = if noise > 0.0 {
                    (self.unit() * 2.0 - 1.0) * noise
                } else {
                    0.0
                };
                s.rgb[i] = (v + n).clamp(0.0, 1.0);
            }
        }

        // ---- 平移（RGB 边缘复制，标签零填充 + valid=0）----
        if self.shift > 0 {
            let lim = self.shift;
            let dx = (self.next_u32() % (2 * lim as u32 + 1)) as i32 - lim;
            let dy = (self.next_u32() % (2 * lim as u32 + 1)) as i32 - lim;
            if dx != 0 || dy != 0 {
                shift_rgb(&mut s.rgb, r, r, dx, dy);
                for pl in &mut s.planes {
                    shift_labels(pl, r, r, dx, dy);
                }
                shift_labels(&mut s.valid, r, r, dx, dy);
            }
        }
    }
}

fn shift_rgb(buf: &mut [f32], w: usize, h: usize, dx: i32, dy: i32) {
    let src = buf.to_vec();
    let clamp = |v: i32, n: usize| v.clamp(0, n as i32 - 1) as usize;
    for y in 0..h {
        for x in 0..w {
            let sx = clamp(x as i32 - dx, w);
            let sy = clamp(y as i32 - dy, h);
            for c in 0..3 {
                buf[(y * w + x) * 3 + c] = src[(sy * w + sx) * 3 + c];
            }
        }
    }
}

/// 标签平移：越界处填 0，并把 `valid` 置 0（不知道标签，不能冒充背景）。
fn shift_labels(buf: &mut [u8], w: usize, h: usize, dx: i32, dy: i32) {
    let src = buf.to_vec();
    for y in 0..h {
        for x in 0..w {
            let sx = x as i32 - dx;
            let sy = y as i32 - dy;
            let inside = sx >= 0 && sx < w as i32 && sy >= 0 && sy < h as i32;
            buf[y * w + x] = if inside {
                src[sy as usize * w + sx as usize]
            } else {
                0
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(res: usize) -> Sample {
        Sample {
            rgb: vec![0.5f32; res * res * 3],
            planes: vec![vec![1u8; res * res]; 2],
            valid: vec![1u8; res * res],
        }
    }

    #[test]
    fn shift_moves_labels_and_invalidates_border() {
        let res = 4usize;
        let mut s = sample(res);
        // 手动平移 +1,0：第 0 列应变成 valid=0
        shift_labels(&mut s.valid, res, res, 1, 0);
        for y in 0..res {
            assert_eq!(s.valid[y * res], 0, "越界列必须 valid=0");
            for x in 1..res {
                assert_eq!(s.valid[y * res + x], 1);
            }
        }
    }

    #[test]
    fn augment_is_deterministic_and_bounded() {
        let res = 8usize;
        let mut a = Augment::new(7, 2);
        let mut b = Augment::new(7, 2);
        let (mut x, mut y) = (sample(res), sample(res));
        a.apply(res as u32, &mut x);
        b.apply(res as u32, &mut y);
        assert_eq!(x.rgb, y.rgb);
        assert!(x.rgb.iter().all(|v| (0.0..=1.0).contains(v)));
    }

    #[test]
    fn shift_zero_is_identity() {
        let res = 5usize;
        let mut a = Augment::new(3, 0);
        let mut s = sample(res);
        let before = (s.rgb.clone(), s.valid.clone(), s.planes.clone());
        a.apply(res as u32, &mut s);
        assert_eq!(s.valid, before.1, "shift=0 时标签必须不动");
        assert_eq!(s.planes, before.2);
        assert_ne!(s.rgb, before.0, "光度扰动仍在（默认开启）");
    }
}
