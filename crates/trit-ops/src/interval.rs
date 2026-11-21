//! 双精度保守区间算术——数值实验的"过闸"地基（rigorous numerics 最小内核）。
//!
//! # 背景声明
//!
//! 这与任何"NS 是否爆破"的声明**无关**：假设态探索线上对 NS 主题的诚实致敬
//! 方式是**治理数值实验**（准入 + 不变量），不是声称定理，工程也不依赖任何
//! 该类声明的真假。本模块是那条治理面的数值底座：给数值实验提供"真值保证
//! 落在算出的区间里"的保守算术，第一个消费方是 `tests/burgers_governance.rs`
//! 的准入检查（CFL 数按区间包络核对）。
//!
//! # 出处（只给名字）
//!
//! 区间算术是标准 rigorous numerics / validated numerics：Moore,
//! *Interval Analysis* (1966) 开创；Moore, Kearfott & Cloud,
//! *Introduction to Interval Analysis* (2009) 是现代教科书处理（端点四则、
//! 单调性、外逼近等表述均取标准定义）；IEEE Std 1788-2015 是区间算术的
//! 标准化文本。本模块是上述标准在 `f64` 上、面向治理语义的一个**保守子集**：
//! 无定向舍入时用 1 ulp 加宽补足（见下），未定义/溢出折叠为 Empty。
//!
//! # 语义决策表（定死，测试钉住）
//!
//! | 情形 | 结果 | 语义 |
//! |---|---|---|
//! | 构造端点 NaN / ±Inf / lo > hi | `EMPTY` | 坏输入在门口折叠，不以合法区间形态存活 |
//! | 任一输入为 `EMPTY` | `EMPTY` | "无保守答案"沿计算链传播 |
//! | add/sub/mul/div 端点溢出 ±Inf（含加宽溢出） | `EMPTY` | 真实结果超出双精度可保守表示的范围，fail-closed |
//! | div 分母含 0（含端点恰为 0） | `EMPTY` | 商集是两段无穷射线的并，单有限区间无法保守表示——拒绝而非返回全区间（全区间 = 信息全丢还假装有答案；Empty 强制调用方显式处理） |
//! | sqrt 定义域违规（lo < 0） | `EMPTY` | 跨出非负定义域不夹逼放行，与 div 含零分母同一取向 |
//! | 其余 | `[min, max]` 各向外加宽 1 ulp | 保守外壳：真值（按 i128 精确有理算术验证）∈ 结果区间 |
//!
//! `EMPTY` 的规范表示是 `lo == hi == NaN`（本模块唯一合法的 NaN 形态）：
//! 活区间恒有 `lo <= hi` 且两端有限。`is_empty()` 是唯一判定入口。
//!
//! # 不变量
//!
//! 1. **非空**：活区间 `lo <= hi`（构造收口保证；运算经 [`Interval::finish`]
//!    收口）。
//! 2. **有限性**：活区间两端有限（NaN/Inf 在构造处 Empty 化；运算端点溢出
//!    Empty 化——包括加宽自身把端点推出 `±f64::MAX` 的情形）。
//! 3. **单调性（包含单调）**：对使运算有定义且结果有限的输入，
//!    `x ⊆ y ⟹ x∘z ⊆ y∘z`（乘法不满足分配律，按区间算术标准定义只承诺
//!    单调性）。**如实界限**：溢出/未定义折叠 Empty 是"中断"语义，不参与
//!    单调性——`x ⊆ y` 且 `y∘z` 折 Empty 时，`x∘z` 的有限结果不被包含；
//!    该例外由 `monotonicity_overflow_exception_is_documented` 钉住。
//!
//! # 加宽（LANDMINE）
//!
//! Rust 的 `f64` 没有定向舍入（IEEE 754 rounding-direction mode 不经 unsafe
//! 或平台内在函数不可达），四则按 round-to-nearest 计算，真值可能落在计算
//! 端点的任一侧 ±0.5 ulp 内。因此**每次运算后**把 lo 向 −∞、hi 向 +∞ 各
//! 加宽 1 ulp（[`widen_down`] / [`widen_up`]，位操作实现），使结果区间
//! 无条件覆盖真值；开销由基准如实报数（`benches/interval_bench.rs` 的
//! 加宽/不加宽对照）。

/// 双精度保守区间。字段私有：不变量（活区间 `lo <= hi` 且有限、Empty 规范
/// 形态 `NaN,NaN`）不可绕过构造入口成立。
#[derive(Debug, Clone, Copy)]
pub struct Interval {
    lo: f64,
    hi: f64,
}

// 命名入口（add/sub/mul/div）是本模块的公开契约：决策表、property 测试、
// 基准与 Burgers 准入全部按名调用与传函数指针；不实现 std::ops 运算符
// trait 是刻意取舍——"Empty 吸收一切"的语义留在显式方法与文档里，不藏在
// 运算符解析后面。
#[allow(clippy::should_implement_trait)]
impl Interval {
    /// Empty（"此步无有限保守答案"）：规范 NaN 形态，与任何运算吸收。
    pub const EMPTY: Interval = Interval {
        lo: f64::NAN,
        hi: f64::NAN,
    };

    /// 构造；NaN / ±Inf / `lo > hi` 一律 Empty 化（决策表第 1 行）。
    /// 倒序区间不解释为反向区间——本 crate 不做 Kaucher/有向区间。
    pub fn new(lo: f64, hi: f64) -> Interval {
        if !lo.is_finite() || !hi.is_finite() || lo > hi {
            return Interval::EMPTY;
        }
        Interval { lo, hi }
    }

    /// 退化区间 `[x, x]`；NaN/Inf 按 [`Interval::new`] 语义 Empty 化。
    pub fn point(x: f64) -> Interval {
        Interval::new(x, x)
    }

    /// π 的夹逼区间：最近 double（`0x4009_21FB_5444_2D18`，3.141592653589793）
    /// 上下各 1 ulp。π = 3.14159265358979323846… 与最近 double 相距 ≤ 0.5 ulp
    /// ⟹ ±1 ulp 必夹住真值（e 同理：2.71828182845904523536…）。
    pub const PI: Interval = Interval {
        lo: widen_down(std::f64::consts::PI),
        hi: widen_up(std::f64::consts::PI),
    };

    /// e 的夹逼区间（构造同 [`Interval::PI`]）。
    pub const E: Interval = Interval {
        lo: widen_down(std::f64::consts::E),
        hi: widen_up(std::f64::consts::E),
    };

    /// 是否 Empty。活区间恒返回 `false`。
    pub const fn is_empty(self) -> bool {
        self.lo.is_nan()
    }

    /// 下端；Empty 时返回 NaN（调用方先过 [`Interval::is_empty`]）。
    pub const fn lo(self) -> f64 {
        self.lo
    }

    /// 上端；Empty 时返回 NaN。
    pub const fn hi(self) -> f64 {
        self.hi
    }

    /// 宽度 `hi - lo`；Empty 为 NaN。
    pub fn width(self) -> f64 {
        if self.is_empty() {
            f64::NAN
        } else {
            self.hi - self.lo
        }
    }

    /// 点包含：`x ∈ [lo, hi]`。Empty 不含任何点（NaN 参与的比较恒假，语义一致）。
    pub fn contains(self, x: f64) -> bool {
        !self.is_empty() && self.lo <= x && x <= self.hi
    }

    /// 子集关系。标准约定：Empty 是任何区间的子集。
    pub fn is_subset(self, other: Interval) -> bool {
        if self.is_empty() {
            return true;
        }
        if other.is_empty() {
            return false;
        }
        other.lo <= self.lo && self.hi <= other.hi
    }

    /// 加法：`[a.lo + b.lo, a.hi + b.hi]`（单调，无需端点组合枚举）。
    pub fn add(self, other: Interval) -> Interval {
        if self.is_empty() || other.is_empty() {
            return Interval::EMPTY;
        }
        Self::finish(self.lo + other.lo, self.hi + other.hi)
    }

    /// 减法：`[a.lo - b.hi, a.hi - b.lo]`。
    pub fn sub(self, other: Interval) -> Interval {
        if self.is_empty() || other.is_empty() {
            return Interval::EMPTY;
        }
        Self::finish(self.lo - other.hi, self.hi - other.lo)
    }

    /// 乘法：四个端点积取 min/max。乘法对区间**不满足分配律**（标准界限，
    /// 出处见模块文档），只按标准定义承诺保守外壳与包含单调。
    pub fn mul(self, other: Interval) -> Interval {
        if self.is_empty() || other.is_empty() {
            return Interval::EMPTY;
        }
        let c = [
            self.lo * other.lo,
            self.lo * other.hi,
            self.hi * other.lo,
            self.hi * other.hi,
        ];
        // 有限输入的乘积只会溢出成 ±Inf，不会产 NaN（0·Inf 不可能出现）。
        let lo = c[0].min(c[1]).min(c[2]).min(c[3]);
        let hi = c[0].max(c[1]).max(c[2]).max(c[3]);
        Self::finish(lo, hi)
    }

    /// 除法。分母区间含 0（含端点恰为 0）→ `EMPTY`（决策表：商集是两段
    /// 无穷射线的并，拒绝而非全区间）；其余四个端点商取 min/max。
    pub fn div(self, other: Interval) -> Interval {
        if self.is_empty() || other.is_empty() {
            return Interval::EMPTY;
        }
        if other.lo <= 0.0 && other.hi >= 0.0 {
            return Interval::EMPTY;
        }
        let c = [
            self.lo / other.lo,
            self.lo / other.hi,
            self.hi / other.lo,
            self.hi / other.hi,
        ];
        let lo = c[0].min(c[1]).min(c[2]).min(c[3]);
        let hi = c[0].max(c[1]).max(c[2]).max(c[3]);
        Self::finish(lo, hi)
    }

    /// 平方根。定义域违规（lo < 0）→ `EMPTY`；其余 `[√lo, √hi]`
    /// （sqrt 单调，IEEE sqrt 对端点正确舍入，1 ulp 加宽覆盖）。
    pub fn sqrt(self) -> Interval {
        if self.is_empty() || self.lo < 0.0 {
            return Interval::EMPTY;
        }
        Self::finish(self.lo.sqrt(), self.hi.sqrt())
    }

    /// 运算出口的唯一收口：先各向外加宽 1 ulp，再过有限性闸。
    /// 任何非有限端点（原始溢出或加宽溢出）→ Empty。
    fn finish(lo: f64, hi: f64) -> Interval {
        let lo = widen_down(lo);
        let hi = widen_up(hi);
        if !lo.is_finite() || !hi.is_finite() {
            return Interval::EMPTY;
        }
        Interval { lo, hi }
    }
}

// LANDMINE：加宽是**正确性代价**，不是精度特性——它把"舍入方向不受控"的
// ≤0.5 ulp 舍入误差主动吃进区间；删掉加宽，所有"真值 ∈ 区间"的保守性断言
// （本模块 property 测试）都可能静默失效。代价的两面：恰在 `±f64::MAX`
// 边界上的精确结果也会因加宽溢出被判 Empty（fail-closed，如实接受）。
//
// 语义：`widen_down(x)` = 严格小于 x 的最大有限 double（向 −∞ 一步）；
// `widen_up(x)` = 严格大于 x 的最小有限 double（向 +∞ 一步）。
// 位操作绕过 ±0 平台（±0 数值相等，跨过去才算真的加宽）；±Inf 饱和（结果
// 端点已是无穷时由调用方的有限性闸 Empty 化，加宽不再推进）。
const fn widen_down(x: f64) -> f64 {
    let bits = x.to_bits();
    let abs = bits & 0x7FFF_FFFF_FFFF_FFFF;
    if abs > 0x7FF0_0000_0000_0000 {
        return x; // NaN 原样传播（EMPTY 形态不在这里固化，由闸收口）
    }
    if bits == 0x7FF0_0000_0000_0000 {
        return f64::MAX; // +Inf 下端：退到最大有限值（仍 ≤ 一切真值）
    }
    if bits == 0xFFF0_0000_0000_0000 {
        return x; // −Inf：向 −∞ 无更小有限值，饱和（有限性闸收口）
    }
    if abs == 0 {
        // ±0 → 负最小 subnormal。裸 bits-1 会绕成 NaN（经典 nextafter 坑）。
        return f64::from_bits(0x8000_0000_0000_0001);
    }
    if x > 0.0 {
        f64::from_bits(bits - 1)
    } else {
        f64::from_bits(bits + 1)
    }
}

/// [`widen_down`] 的镜像：严格大于 x 的最小有限 double（向 +∞ 一步）。
const fn widen_up(x: f64) -> f64 {
    let bits = x.to_bits();
    let abs = bits & 0x7FFF_FFFF_FFFF_FFFF;
    if abs > 0x7FF0_0000_0000_0000 {
        return x;
    }
    if bits == 0x7FF0_0000_0000_0000 {
        return x; // +Inf：向 +∞ 无更大有限值，饱和
    }
    if bits == 0xFFF0_0000_0000_0000 {
        return f64::MIN; // −Inf 上端：退到最小有限值
    }
    if abs == 0 {
        return f64::from_bits(1); // ±0 → 正最小 subnormal（5e−324；MIN_POSITIVE 是最小"正规"数，会跳过全部 subnormal）
    }
    if x > 0.0 {
        f64::from_bits(bits + 1)
    } else {
        f64::from_bits(bits - 1)
    }
}

// NaN,NaN 的 EMPTY 使 `#[derive(PartialEq)]` 静默破坏等值（NaN != NaN，
// Empty == Empty 会判 false）——手工实现，Empty 按形态相等。
impl PartialEq for Interval {
    fn eq(&self, other: &Self) -> bool {
        if self.is_empty() || other.is_empty() {
            self.is_empty() == other.is_empty()
        } else {
            self.lo == other.lo && self.hi == other.hi
        }
    }
}
impl Eq for Interval {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    /// 确定 xorshift64*（同 gf3/frame 的随机测试口径，零新依赖）。
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
    }

    // ---------- 精确真值对照（i128 有理算术） ----------

    /// 随机精确 dyadic：v = k/64，k ∈ [−2^20, 2^20]（f64 精确表示，
    /// 真值 = num64/64 在 i128 里精确）。返回 (精确 f64, num64)。
    fn dyadic(rng: &mut Rng) -> (f64, i128) {
        let k = (rng.next() % (1 << 21)) as i128 - (1 << 20);
        (k as f64 / 64.0, k)
    }

    /// 随机非空区间（端点取自 dyadic，1/4 概率退化为点区间）。
    /// 返回 (区间, lo 的 num64, hi 的 num64)——真值 = num64/64。
    fn rand_interval(rng: &mut Rng) -> (Interval, i128, i128) {
        let (v1, n1) = dyadic(rng);
        let (v2, n2) = dyadic(rng);
        if rng.next().is_multiple_of(4) {
            return (Interval::new(v1, v1), n1, n1);
        }
        if v1 <= v2 {
            (Interval::new(v1, v2), n1, n2)
        } else {
            (Interval::new(v2, v1), n2, n1)
        }
    }

    /// 同号嵌套除数对（y ⊇ x，两端同号非零；真值 = num/64）。
    /// 右参单调 z/x ⊆ z/y 的核对需要 y 也排除 0——含零折叠是中断语义
    /// （模块文档不变量 3 的同一例外）。
    fn nested_divisor_pair(rng: &mut Rng) -> (Interval, Interval) {
        let sign = if rng.next().is_multiple_of(2) {
            1i128
        } else {
            -1
        };
        let k1 = 1 + (rng.next() % (1 << 18)) as i128;
        let k2 = 1 + (rng.next() % (1 << 18)) as i128;
        let k3 = 1 + (rng.next() % (1 << 18)) as i128;
        let k4 = 1 + (rng.next() % (1 << 18)) as i128;
        let (xlo, xhi) = (k1.min(k2), k1.max(k2));
        let (ylo, yhi) = (xlo.min(k3), xhi.max(k4));
        let (xl, xh, yl, yh) = if sign > 0 {
            (xlo, xhi, ylo, yhi)
        } else {
            (-xhi, -xlo, -yhi, -ylo)
        };
        let v = |k: i128| k as f64 / 64.0;
        (Interval::new(v(xl), v(xh)), Interval::new(v(yl), v(yh)))
    }

    /// 除数区间：两端同号（排除 0），真值 = num64/64，|num64| ∈ [1, 2^18]。
    fn rand_divisor(rng: &mut Rng) -> (Interval, i128, i128) {
        let sign = if rng.next().is_multiple_of(2) {
            1i128
        } else {
            -1
        };
        let k1 = 1 + (rng.next() % (1 << 18)) as i128;
        let k2 = 1 + (rng.next() % (1 << 18)) as i128;
        let (kl, kh) = if k1 <= k2 { (k1, k2) } else { (k2, k1) };
        let (nl, nh) = if sign > 0 { (kl, kh) } else { (-kh, -kl) };
        let v = |num: i128| num as f64 / 64.0;
        (Interval::new(v(nl), v(nh)), nl, nh)
    }

    /// 精确比较真值 t = p/q（q > 0，|p| ≤ 2^52，q ≤ 2^26）与端点 x。
    ///
    /// - x 为 ±0 或 subnormal：本测试非零真值 \|t\| ≥ 2^−26，而 subnormal
    ///   端点只可能出现在真值恰为 0（加宽跨 ±0 平台）时——量级差 ≥ 2^996，
    ///   符号即判决；
    /// - 其余（同量级，x ≈ t）：交叉相乘 \|p\|·2^e vs m·q，两侧代表同一量级
    ///   （x ≈ t ⟹ \|p\|·2^e ≈ m·q ≤ 2^53·2^26 = 2^79），i128 无溢出。
    fn cmp_true(p: i128, q: i128, x: f64) -> Ordering {
        debug_assert!(q > 0, "分母必须已归一为正");
        let abs = x.to_bits() & 0x7FFF_FFFF_FFFF_FFFF;
        if abs == 0 || abs < 0x0010_0000_0000_0000 {
            // x = ±0 或 subnormal：量级悬殊，符号即判决（见函数注释）。
            if p == 0 {
                return if x == 0.0 {
                    Ordering::Equal
                } else if x < 0.0 {
                    Ordering::Greater
                } else {
                    Ordering::Less
                };
            }
            return if p > 0 {
                Ordering::Greater
            } else {
                Ordering::Less
            };
        }
        if p == 0 {
            return if x > 0.0 {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        let t_neg = p < 0;
        let x_neg = (x.to_bits() >> 63) == 1;
        if t_neg != x_neg {
            // 符号相反：正的一侧大。
            return if t_neg {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        // 分解 x = ±m·2^−e（m ∈ [2^52, 2^53)）。
        let biased = ((x.to_bits() >> 52) & 0x7FF) as i128;
        let m = ((x.to_bits() & 0x000F_FFFF_FFFF_FFFF) as i128) | (1 << 52);
        let e = 1075 - biased;
        debug_assert!((0..=110).contains(&e), "测试值域内端点指数应有界：{e}");
        // |p|·2^e vs m·q（同号侧比较，两侧 ≈ |t|·q·2^e，同量级）。
        let lhs = p.abs().checked_shl(e as u32).expect("同量级交叉乘积不溢出");
        let rhs = m.checked_mul(q).expect("尾数×分母不溢出");
        let ord = lhs.cmp(&rhs);
        if t_neg { ord.reverse() } else { ord }
    }

    /// 断言真值区间 [tlo, thi]（num/den 形式）被 c 保守包含且 c 活着。
    fn assert_contains(c: Interval, tlo: i128, thi: i128, den: i128) {
        assert!(!c.is_empty(), "有限域内的定义运算不得折叠 Empty");
        assert!(c.lo().is_finite() && c.hi().is_finite(), "端点必须有限");
        assert_ne!(
            cmp_true(tlo, den, c.lo()),
            Ordering::Less,
            "c.lo 不得超过真值下端"
        );
        assert_ne!(
            cmp_true(thi, den, c.hi()),
            Ordering::Greater,
            "c.hi 不得小于真值上端"
        );
    }

    /// 包含单调：x ⊆ y ⟹ x∘z ⊆ y∘z 且 z∘x ⊆ z∘y（对二元 op）。
    fn assert_monotone(
        x: Interval,
        y: Interval,
        z: Interval,
        op: fn(Interval, Interval) -> Interval,
    ) {
        assert!(x.is_subset(y), "生成器保证 x ⊆ y");
        assert!(op(x, z).is_subset(op(y, z)), "x⊆y ⟹ x∘z ⊆ y∘z（左参）");
        assert!(op(z, x).is_subset(op(z, y)), "x⊆y ⟹ z∘x ⊆ z∘y（右参）");
    }

    /// 从随机区间对构造 y ⊇ x（取两区间并集外壳；真值 = num/64 精确）。
    fn nested_pair(rng: &mut Rng) -> (Interval, Interval) {
        let (x, xl, xh) = rand_interval(rng);
        let (_, yl, yh) = rand_interval(rng);
        let y = Interval::new(xl.min(yl) as f64 / 64.0, xh.max(yh) as f64 / 64.0);
        (x, y)
    }

    // ---------- property：1 万例/运算，真值对照 + 包含单调 ----------

    #[test]
    fn property_add_containment_and_monotonicity() {
        let mut rng = Rng(0xA11C_E5C0_0000_0001);
        for _ in 0..10_000 {
            let (a, al, ah) = rand_interval(&mut rng);
            let (b, bl, bh) = rand_interval(&mut rng);
            assert_contains(a.add(b), al + bl, ah + bh, 64);
            let (x, y) = nested_pair(&mut rng);
            assert_monotone(x, y, b, Interval::add);
        }
    }

    #[test]
    fn property_sub_containment_and_monotonicity() {
        let mut rng = Rng(0xA11C_E5C0_0000_0002);
        for _ in 0..10_000 {
            let (a, al, ah) = rand_interval(&mut rng);
            let (b, bl, bh) = rand_interval(&mut rng);
            assert_contains(a.sub(b), al - bh, ah - bl, 64);
            let (x, y) = nested_pair(&mut rng);
            assert_monotone(x, y, b, Interval::sub);
        }
    }

    #[test]
    fn property_mul_containment_and_monotonicity() {
        let mut rng = Rng(0xA11C_E5C0_0000_0003);
        for _ in 0..10_000 {
            let (a, al, ah) = rand_interval(&mut rng);
            let (b, bl, bh) = rand_interval(&mut rng);
            let c = a.mul(b);
            // 真值集 = {p·q} ⊆ 四个端点积的外壳（双线性单调），逐个精确核对。
            for p in [al * bl, al * bh, ah * bl, ah * bh] {
                assert!(
                    cmp_true(p, 4096, c.lo()) != Ordering::Less
                        && cmp_true(p, 4096, c.hi()) != Ordering::Greater,
                    "端点积 {p}/4096 必须落在结果内"
                );
            }
            let (x, y) = nested_pair(&mut rng);
            assert_monotone(x, y, b, Interval::mul);
        }
    }

    #[test]
    fn property_div_containment_and_monotonicity() {
        let mut rng = Rng(0xA11C_E5C0_0000_0004);
        for _ in 0..10_000 {
            let (a, al, ah) = rand_interval(&mut rng);
            let (b, bl, bh) = rand_divisor(&mut rng);
            let c = a.div(b);
            assert!(!c.is_empty(), "同号非零除数的除法必须有定义");
            // 真值集 ⊆ 四个端点商的外壳（b 同号 ⟹ 1/b 单调）。
            // 商 al/bl 按 q = |bl| > 0 归一（符号折进分子）。
            for (na, nb) in [(al, bl), (al, bh), (ah, bl), (ah, bh)] {
                let (p, q) = (na * nb.signum(), nb.abs());
                assert!(
                    cmp_true(p, q, c.lo()) != Ordering::Less
                        && cmp_true(p, q, c.hi()) != Ordering::Greater,
                    "端点商 {p}/{q} 必须落在结果内"
                );
            }
            // 单调（左参）：除数 b 非零 ⟹ x/b 与 y/b 都有定义。
            let (x, y) = nested_pair(&mut rng);
            assert!(x.div(b).is_subset(y.div(b)), "x⊆y ⟹ x/b ⊆ y/b（左参）");
            // 单调（右参）：x ⊆ y 同号非零 ⟹ 1/x 的像 ⊆ 1/y 的像（1/x 递减，
            // 定义域扩大两端外推）。y 含零时 z/y 折 Empty 是中断语义，不核对。
            let (z, _, _) = rand_interval(&mut rng);
            let (dx, dy) = nested_divisor_pair(&mut rng);
            assert!(
                z.div(dx).is_subset(z.div(dy)),
                "x⊆y（同号非零）⟹ z/x ⊆ z/y（右参）"
            );
        }
    }

    #[test]
    fn property_sqrt_containment_and_monotonicity() {
        // 输入限 [2^−4, 2^12]（k/16，k ∈ [1, 2^16]）：结果 ∈ [2^−2, 2^6]，
        // c² 与真值的交叉乘积 ≤ 2^124，i128 精确可行（见 cmp_true 注释）。
        let mut rng = Rng(0xA11C_E5C0_0000_0005);
        for _ in 0..10_000 {
            let k1 = 1 + (rng.next() % (1 << 16)) as i128;
            let k2 = 1 + (rng.next() % (1 << 16)) as i128;
            let (kl, kh) = (k1.min(k2), k1.max(k2));
            let c = Interval::new(kl as f64 / 16.0, kh as f64 / 16.0).sqrt();
            assert!(!c.is_empty());
            // c.lo ≤ √lo ⟺ (c.lo ≤ 0) ∨ c.lo² ≤ lo；c.hi ≥ √hi ⟺ hi ≤ c.hi²。
            let clo = c.lo();
            if clo > 0.0 {
                let (m, e) = dyadic_decomp(clo);
                assert!(m * m * 16 <= kl << (2 * e), "c.lo² ≤ lo 必须精确成立");
            }
            let (m, e) = dyadic_decomp(c.hi());
            assert!(kh << (2 * e) <= m * m * 16, "hi ≤ c.hi² 必须精确成立");
            // 单调：上端扩张 ⟹ sqrt 结果仍包含。
            let k3 = 1 + (rng.next() % (1 << 16)) as i128;
            let y = Interval::new(kl as f64 / 16.0, kh.max(k3) as f64 / 16.0).sqrt();
            assert!(c.is_subset(y), "sqrt 包含单调（扩上端）");
        }
    }

    /// 把非零正 f64 分解为 m·2^−e（m ∈ [2^52, 2^53)），供 sqrt 精确对照平方用。
    fn dyadic_decomp(x: f64) -> (i128, u32) {
        let bits = x.to_bits();
        let biased = ((bits >> 52) & 0x7FF) as i128;
        let m = ((bits & 0x000F_FFFF_FFFF_FFFF) as i128) | (1 << 52);
        (m, (1075 - biased) as u32)
    }

    // ---------- 确定性语义（决策表逐行钉住） ----------

    #[test]
    fn construction_sanitizes_nan_inf_and_inversion() {
        assert!(Interval::new(f64::NAN, 1.0).is_empty());
        assert!(Interval::new(1.0, f64::NAN).is_empty());
        assert!(Interval::new(f64::INFINITY, 1.0).is_empty());
        assert!(Interval::new(1.0, f64::NEG_INFINITY).is_empty());
        assert!(Interval::new(2.0, 1.0).is_empty(), "倒序区间 Empty 化");
        assert!(Interval::point(f64::NAN).is_empty());
        let live = Interval::new(1.0, 2.0);
        assert!(!live.is_empty());
        assert_eq!(live.lo(), 1.0);
        assert_eq!(live.hi(), 2.0);
        assert_eq!(Interval::point(3.5).width(), 0.0);
        assert!(Interval::point(3.5).contains(3.5));
        assert!(!Interval::point(3.5).contains(3.6));
    }

    #[test]
    fn empty_propagates_through_all_ops_and_equality_works() {
        // NaN,NaN 形态下 derive(PartialEq) 会判 Empty != Empty——手工实现钉住。
        assert_eq!(Interval::EMPTY, Interval::EMPTY);
        let live = Interval::new(1.0, 2.0);
        assert_ne!(Interval::EMPTY, live);
        assert_ne!(live, Interval::new(1.0, 3.0));
        assert_eq!(live, Interval::new(1.0, 2.0));
        for r in [
            Interval::EMPTY.add(live),
            live.add(Interval::EMPTY),
            Interval::EMPTY.sub(live),
            Interval::EMPTY.mul(live),
            Interval::EMPTY.div(live),
            Interval::EMPTY.sqrt(),
        ] {
            assert!(r.is_empty(), "Empty 必须吸收一切运算");
        }
        // 子集语义：Empty ⊆ 任意；活区间 ⊄ Empty；Empty 不含点。
        assert!(Interval::EMPTY.is_subset(live));
        assert!(!live.is_subset(Interval::EMPTY));
        assert!(!Interval::EMPTY.contains(1.0));
        assert!(live.is_subset(Interval::new(0.0, 3.0)));
        assert!(!Interval::new(0.0, 3.0).is_subset(live));
    }

    #[test]
    fn exact_tight_cases_match_single_ulp_widening() {
        // 端点积/和精确可表示时，结果 = 精确端点各加宽 1 ulp（不更多）。
        let c = Interval::new(1.0, 2.0).add(Interval::new(3.0, 4.0));
        assert_eq!(c.lo(), widen_down(4.0));
        assert_eq!(c.hi(), widen_up(6.0));
        let c = Interval::new(1.0, 2.0).mul(Interval::new(1.0, 2.0));
        assert_eq!(c.lo(), widen_down(1.0));
        assert_eq!(c.hi(), widen_up(4.0));
        // 1/3 不可精确表示：lo 必须是 fl(1/3) 再向下 1 ulp，且 1/3 在区间内。
        let c = Interval::new(1.0, 1.0).div(Interval::new(3.0, 3.0));
        assert_eq!(c.lo(), widen_down(1.0 / 3.0));
        assert_eq!(c.hi(), widen_up(1.0 / 3.0));
        assert!(c.contains(1.0 / 3.0));
        // sqrt(4,9) 端点精确：[widen_down(2), widen_up(3)]。
        let c = Interval::new(4.0, 9.0).sqrt();
        assert_eq!(c.lo(), widen_down(2.0));
        assert_eq!(c.hi(), widen_up(3.0));
        assert!(c.contains(2.5));
    }

    #[test]
    // 十进制近似常数是刻意的：松界取远离 π/e 舍入边界的整十进制位，
    // 断言"夹逼区间 ⊆ 十进制松界"不受端点具体 bit 形态影响。
    #[allow(clippy::approx_constant)]
    fn constants_bracket_pi_and_e() {
        // 最近 double 距真值 ≤ 0.5 ulp ⟹ ±1 ulp 夹逼必然成立。
        for (c, mid, loose_lo, loose_hi) in [
            (Interval::PI, std::f64::consts::PI, 3.141592653, 3.141592654),
            (Interval::E, std::f64::consts::E, 2.718281828, 2.718281829),
        ] {
            assert!(!c.is_empty());
            assert!(c.contains(mid), "最近 double 必须在夹逼区间内");
            let mid_bits = mid.to_bits();
            assert_eq!(c.lo().to_bits(), mid_bits - 1, "下端恰 1 ulp");
            assert_eq!(c.hi().to_bits(), mid_bits + 1, "上端恰 1 ulp");
            assert!(
                c.is_subset(Interval::new(loose_lo, loose_hi)),
                "十进制松界包住"
            );
        }
        assert!(Interval::PI.contains(3.141592653589793));
        assert!(Interval::E.contains(2.718281828459045));
    }

    #[test]
    fn division_and_sqrt_domain_fail_closed() {
        // 含零分母：内含、端点 0、点 [0,0] 三形态全拒。
        assert!(
            Interval::new(1.0, 2.0)
                .div(Interval::new(-1.0, 1.0))
                .is_empty()
        );
        assert!(
            Interval::new(1.0, 2.0)
                .div(Interval::new(0.0, 1.0))
                .is_empty()
        );
        assert!(
            Interval::new(1.0, 2.0)
                .div(Interval::new(-1.0, 0.0))
                .is_empty()
        );
        assert!(Interval::new(1.0, 2.0).div(Interval::point(0.0)).is_empty());
        // 同号分母有定义：[1,2]/[−2,−1] = [−2, −0.5]（各加宽 1 ulp）。
        let c = Interval::new(1.0, 2.0).div(Interval::new(-2.0, -1.0));
        assert_eq!(c.lo(), widen_down(-2.0));
        assert_eq!(c.hi(), widen_up(-0.5));
        assert!(c.contains(-1.5));
        // sqrt 定义域：负 → Empty；−0.0 不是负（== 0.0）；非负域有定义。
        assert!(Interval::new(-1.0, 4.0).sqrt().is_empty());
        assert!(Interval::new(-4.0, -1.0).sqrt().is_empty());
        assert!(!Interval::new(0.0, 4.0).sqrt().is_empty());
        assert!(!Interval::point(-0.0).sqrt().is_empty());
    }

    #[test]
    fn overflow_and_widen_overflow_fail_closed() {
        // 原始溢出：端点积/和超出 ±f64::MAX。
        assert!(
            Interval::new(f64::MAX, f64::MAX)
                .mul(Interval::new(2.0, 2.0))
                .is_empty()
        );
        assert!(
            Interval::new(f64::MAX, f64::MAX)
                .add(Interval::new(1.0, 1.0))
                .is_empty()
        );
        // 加宽溢出：−MAX−1 舍入回 −MAX，再向 −∞ 加宽无有限值可去——真实
        // 结果 −MAX−1 不可保守表示 → Empty（fail-closed 的对称面）。
        assert!(
            Interval::new(f64::MIN, f64::MIN)
                .add(Interval::new(-1.0, -1.0))
                .is_empty()
        );
        // sqrt 收敛不溢出（对照：MAX 的 sqrt ≈ 1.3e154，有限）。
        assert!(!Interval::new(f64::MAX, f64::MAX).sqrt().is_empty());
        // 乘 [1,1] 端点恰为 MAX：hi 加宽出界 → Empty（加宽无条件，见 widening 测试）。
        assert!(
            Interval::new(f64::MAX, f64::MAX)
                .mul(Interval::new(1.0, 1.0))
                .is_empty()
        );
    }

    #[test]
    fn monotonicity_overflow_exception_is_documented() {
        // 如实界限（模块文档不变量 3）：溢出折叠 Empty 是中断语义，不参与
        // 单调性——x ⊆ y 且 y∘z 溢出时 x∘z 的有限结果不被包含。
        let x = Interval::new(1e300, 1e300);
        let y = Interval::new(1e300, f64::MAX);
        assert!(x.is_subset(y));
        let xz = x.mul(Interval::new(2.0, 2.0));
        let yz = y.mul(Interval::new(2.0, 2.0));
        assert!(!xz.is_empty());
        assert!(yz.is_empty(), "MAX·2 溢出 → Empty");
        assert!(!xz.is_subset(yz), "溢出例外：有限结果不被 Empty 包含");
    }

    #[test]
    fn zero_width_interval_widens_across_signed_zero_plateau() {
        // [0,0]·[−1,1]：原始端点 ±0（数值相等）。裸 bits±1 的 nextafter 在
        // ±0 处会绕成 NaN 或原地不动——加宽必须跨过带符号零平台，落到
        // 最小 subnormal（5e−324），而不是最小正规数（MIN_POSITIVE，中间
        // 隔着 2^52 个 subnormal）。
        let c = Interval::point(0.0).mul(Interval::new(-1.0, 1.0));
        assert_eq!(
            c.lo(),
            f64::from_bits(0x8000_0000_0000_0001),
            "lo 跨到负最小 subnormal"
        );
        assert_eq!(c.hi(), f64::from_bits(1), "hi 跨到正最小 subnormal");
        // 点区间自加同样跨平台。
        let c = Interval::point(0.0).add(Interval::point(0.0));
        assert!(c.lo() < 0.0 && c.hi() > 0.0);
        assert_eq!(c.lo(), f64::from_bits(0x8000_0000_0000_0001));
        assert_eq!(c.hi(), f64::from_bits(1));
    }

    #[test]
    fn widening_applies_even_to_exact_results() {
        // [1,1]+[1,1] 精确等于 [2,2]，仍加宽 1 ulp——加宽无条件（正确性代价，
        // 不是按需优化）：代价是边界处折 Empty（见 overflow 测试），收益是
        // 保守性不依赖对"这次运算是否精确"的判断。
        let c = Interval::point(1.0).add(Interval::point(1.0));
        assert!(c.lo() < 2.0 && c.hi() > 2.0);
        assert_eq!(c.lo(), widen_down(2.0));
        assert_eq!(c.hi(), widen_up(2.0));
    }
}
