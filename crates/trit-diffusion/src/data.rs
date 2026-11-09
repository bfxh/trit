//! 可判定的 2D 数据集与确定性 PRNG。
//!
//! 生成模型最容易糊弄自己的地方是「看起来像」。这里用**两个分离模态**的
//! 解析数据（半径 1 的细圆环 + 半径 0.25 的均匀圆盘，各 50%），采样后
//! 直接统计「落在环上的比例 / 落在盘内的比例」，模态塌缩会立刻暴露
//! （扩散模型最常见的失败模式就是只生成一个模态）。

/// xorshift64* PRNG：确定性、可复算，不引第三方 rng。
#[derive(Clone, Debug)]
pub struct Xorshift {
    state: u64,
}

impl Xorshift {
    /// 新建（种子 0 会被抬成 1，避免全零状态）。
    pub fn new(seed: u64) -> Self {
        Self { state: seed | 1 }
    }

    /// 原始 64 位输出。
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// `[0, 1)` 均匀分布。
    pub fn next_f32(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32) / ((1u32 << 24) as f32)
    }

    /// 标准正态（Box-Muller）。
    pub fn normal(&mut self) -> f32 {
        let u1 = self.next_f32().max(1e-7);
        let u2 = self.next_f32();
        let r = (-2.0 * (u1 as f64).ln()).sqrt();
        (r * (std::f64::consts::TAU * u2 as f64).cos()) as f32
    }
}

/// 采样 2D 两模态数据：50% 半径 1 的细环（径向抖动 0.08），50% 半径 0.25 的均匀盘。
///
/// 输出在 `[-1.2, 1.2]²` 内，直接当作扩散模型的 `x₀`。
pub fn sample_ring(n: usize, rng: &mut Xorshift) -> Vec<[f32; 2]> {
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let angle = std::f64::consts::TAU * rng.next_f32() as f64;
        let (cx, cy) = (angle.cos() as f32, angle.sin() as f32);
        if i % 2 == 0 {
            // 环：r = 1 + N(0, 0.08)
            let r = 1.0 + 0.08 * rng.normal();
            out.push([r * cx, r * cy]);
        } else {
            // 盘：均匀面积采样 ⇒ r = R·√u
            let r = 0.25 * rng.next_f32().sqrt();
            out.push([r * cx, r * cy]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prng_is_deterministic() {
        let mut a = Xorshift::new(7);
        let mut b = Xorshift::new(7);
        for _ in 0..16 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn normal_has_reasonable_moments() {
        let mut rng = Xorshift::new(11);
        let n = 20_000;
        let mut sum = 0.0f64;
        let mut sq = 0.0f64;
        for _ in 0..n {
            let z = rng.normal() as f64;
            sum += z;
            sq += z * z;
        }
        let mean = sum / n as f64;
        let var = sq / n as f64 - mean * mean;
        assert!(mean.abs() < 0.03, "均值偏差过大：{mean}");
        assert!((var - 1.0).abs() < 0.06, "方差偏差过大：{var}");
    }

    #[test]
    fn ring_data_has_two_modes() {
        let mut rng = Xorshift::new(3);
        let pts = sample_ring(2000, &mut rng);
        let ring = pts
            .iter()
            .filter(|p| {
                let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
                (r - 1.0).abs() < 0.2
            })
            .count();
        let disk = pts
            .iter()
            .filter(|p| (p[0] * p[0] + p[1] * p[1]).sqrt() < 0.3)
            .count();
        assert!(
            ring > 900 && disk > 900,
            "两模态应各占约一半：ring={ring} disk={disk}"
        );
    }
}
