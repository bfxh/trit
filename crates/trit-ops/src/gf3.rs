//! GF(3) 域算术与 q-ary Hamming 码（trit 级纠错）。
//!
//! # 背景声明
//!
//! 这与黎曼猜想**无关**：RH 是素数分布定理，对内核构造没有任何推论。
//! 动机是三进制硬件路线下的 **trit 级存储完整性**——ECC 内存的 q-ary 对应物。
//! 数学依据是标准 q-ary 编码理论：GF(3) 上的 q-ary Hamming 码（r 个校验位 →
//! 长度 `n = (3^r - 1)/2` 的单纠错码）与三元 Golay `[11,6,5]`（见
//! [`crate::golay`]）。出处只给名字：Hamming (1950)；q-ary 射影构造与
//! 三元 Golay 见 MacWilliams & Sloane, *The Theory of Error-Correcting Codes*。
//!
//! # 表示（用本仓自己的，不新造）
//!
//! 平衡三值 [`Trit`] ∈ `{-1, 0, +1}`：GF(3) 乘法就是 [`Trit::mul`]（`±1`
//! 闭包），加法按 mod 3 环回（`+2 ≡ -1`、`-2 ≡ +1`）。注意 [`Trit::add_sat`]
//! 是**饱和**加法，不是 GF(3) 加法。标准位 `{0,1,2}` 经 [`digit`] /
//! [`from_digit`] 互转（平衡 `-1` ↔ 标准 `2`）。
//!
//! # 码构造与界限（如实写，不夸大）
//!
//! 校验矩阵 `H` 的列取 GF(3)^r 射影空间 PG(r-1,3) 的**每条一维子空间一个
//! 代表元**（首非零坐标归一为 1），末 r 列为单位向量 → 系统形
//! `H = [H_A | I_r]`，生成矩阵 `G = [I_k | -H_A^T]`，`r=3 → [13,10,3]`、
//! `r=4 → [40,36,3]`。最小距离 3：**保证纠正 1 个 trit 错**；伴随式对双错
//! 恒非零（双错永不静默通过），但译码器无法区分 1 错与 2 错——2 错会被
//! "纠正"成另一个码字（系统性测试实测 100% 误纠）。要"纠 1 检 2"用
//! [`ExtendedHamming`]（整体校验位扩展，d=4，教科书 SECDED 构造；双错
//! 判 Suspect 的比率是构造相关常数，测试实测钉死），要**确定性**的
//! 单错自愈/双错拒绝用三元 Golay（d=5，见 [`crate::frame`] 的消费语义）。

use crate::trit::Trit;

/// 平衡 trit 的最大校验位数（本模块支持 r=3 与 r=4）。
const MAX_R: usize = 4;

/// GF(3) 加法（平衡表示）：`+2 ≡ -1`、`-2 ≡ +1`（mod 3 环回）。
#[inline]
pub const fn add(a: Trit, b: Trit) -> Trit {
    match a.get() + b.get() {
        2 => Trit::NEG,
        -2 => Trit::POS,
        s => match s {
            1 => Trit::POS,
            -1 => Trit::NEG,
            _ => Trit::ZERO,
        },
    }
}

/// GF(3) 减法。
#[inline]
pub const fn sub(a: Trit, b: Trit) -> Trit {
    add(a, neg(b))
}

/// GF(3) 取负（平衡表示下 `-1 → +1`，即标准位 `1 ↔ 2`）。
#[inline]
pub const fn neg(a: Trit) -> Trit {
    a.neg()
}

/// GF(3) 乘法：平衡表示下与 [`Trit::mul`] 重合（`±1 * ±1 = ±1`，闭包）。
#[inline]
pub const fn mul(a: Trit, b: Trit) -> Trit {
    a.mul(b)
}

/// 平衡 trit → 标准位 `{0,1,2}`（`-1 → 2`）。
#[inline]
pub fn digit(t: Trit) -> u8 {
    (t.get() + 3).rem_euclid(3) as u8
}

/// 标准位 `{0,1,2}` → 平衡 trit（`2 → -1`）；越界按 mod 3 归约。
#[inline]
pub fn from_digit(d: u8) -> Trit {
    match d % 3 {
        0 => Trit::ZERO,
        1 => Trit::POS,
        _ => Trit::NEG,
    }
}

/// GF(3) 内积（平衡算术），长度必须一致。校验矩阵乘/伴随式都靠它。
#[inline]
pub fn dot_mod3(x: &[Trit], y: &[Trit]) -> Trit {
    let mut acc = Trit::ZERO;
    for (a, b) in x.iter().zip(y.iter()) {
        acc = add(acc, mul(*a, *b));
    }
    acc
}

/// 一次 Hamming 译码结果。
///
/// 纯 Hamming（d=3）只有两态：伴随式为 0 = 干净；非 0 = 按 1 错纠正——
/// 对真双错这是**误纠**（见模块文档），调用方不能把它当"确定修好"。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HammingDecode {
    /// 伴随式为 0（≥3 错可能伪装成干净，d=3 的固有界限）。
    Clean,
    /// 按单错纠正；`magnitude` 是标准位 `1`/`2`。
    Corrected { position: usize, magnitude: u8 },
}

/// q-ary Hamming 码（q=3）。支持 r=3（`[13,10,3]`）与 r=4（`[40,36,3]`）。
///
/// 列 = PG(r-1,3) 全部射影点各取一个首非零归一代表元，末 r 列为单位向量，
/// 即系统形 `H = [H_A | I_r]`；校验块 = `-(H_A · m)`（[`encode_into`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HammingCode {
    r: usize,
    n: usize,
    k: usize,
    /// n 列校验矩阵，每列 r 个平衡 trit（列优先存储）。
    cols: Vec<[Trit; MAX_R]>,
    /// `3^r` 项伴随式表：打包索引 → `(位置, 幅度标准位)`；索引 0 恒 `None`。
    table: Vec<Option<(u8, u8)>>,
}

impl HammingCode {
    /// `r = 3`：`[13,10,3]`。
    pub fn r3() -> Self {
        Self::new(3).expect("r=3 必然可构造")
    }

    /// `r = 4`：`[40,36,3]`。
    pub fn r4() -> Self {
        Self::new(4).expect("r=4 必然可构造")
    }

    /// 按校验位数构造；只支持 3 与 4（`r=2` 的 `[4,2,3]` 玩具码在 trit-codec）。
    pub fn new(r: u32) -> Option<Self> {
        if r < 3 || r as usize > MAX_R {
            return None;
        }
        let r = r as usize;
        let n = (3usize.pow(r as u32) - 1) / 2;
        let k = n - r;
        // 射影代表元：首非零坐标归一为 1，去重后剔除单位向量（它们当 I_r）。
        let mut canonical: Vec<Vec<u8>> = Vec::with_capacity(n);
        for raw in 0..3u32.pow(r as u32) {
            let v: Vec<u8> = (0..r)
                .map(|i| ((raw / 3u32.pow(i as u32)) % 3) as u8)
                .collect();
            if v.iter().all(|&d| d == 0) {
                continue;
            }
            // 首非零坐标归一为 1：系数 1 → 不动，2 → 乘逆元 2。
            let (lead, scale) = {
                let i = v.iter().position(|&d| d != 0).expect("非零向量有首非零");
                (i, if v[i] == 1 { 1u8 } else { 2 })
            };
            let norm: Vec<u8> = v
                .iter()
                .enumerate()
                .map(|(i, &d)| if i < lead { d } else { (d * scale) % 3 })
                .collect();
            if !canonical.contains(&norm) {
                canonical.push(norm);
            }
        }
        let unit = |j: usize| {
            (0..r)
                .map(|i| if i == j { 1u8 } else { 0 })
                .collect::<Vec<u8>>()
        };
        let units: Vec<Vec<u8>> = (0..r).map(unit).collect();
        let mut nonunit: Vec<Vec<u8>> = canonical
            .into_iter()
            .filter(|c| !units.contains(c))
            .collect();
        nonunit.sort();
        debug_assert_eq!(nonunit.len(), k);
        let mut digits: Vec<Vec<u8>> = nonunit;
        digits.extend(units);
        debug_assert_eq!(digits.len(), n);
        let cols: Vec<[Trit; MAX_R]> = digits
            .iter()
            .map(|c| {
                let mut col = [Trit::ZERO; MAX_R];
                for (i, &d) in c.iter().enumerate() {
                    col[i] = from_digit(d);
                }
                col
            })
            .collect();
        // 伴随式表：单错型 (位置, 幅度) → 打包伴随式。射影完备 ⇒ 非零伴随式全覆盖。
        let mut table = vec![None; 3usize.pow(r as u32)];
        for pos in 0..n {
            for mag in 1..=2u8 {
                let mut e = vec![Trit::ZERO; n];
                e[pos] = from_digit(mag);
                let idx = syndrome_index(&cols, r, &e);
                debug_assert_ne!(idx, 0, "单错伴随式不能为 0");
                debug_assert!(table[idx as usize].is_none(), "伴随式必须双射");
                table[idx as usize] = Some((pos as u8, mag));
            }
        }
        debug_assert!(
            table.iter().as_slice()[1..]
                .iter()
                .all(|slot| slot.is_some()),
            "射影完备 ⇒ 每个非零伴随式命中一列"
        );
        Some(Self {
            r,
            n,
            k,
            cols,
            table,
        })
    }

    /// 码字长度 n。
    pub fn n(&self) -> usize {
        self.n
    }

    /// 消息长度 k。
    pub fn k(&self) -> usize {
        self.k
    }

    /// 校验位数 r。
    pub fn r(&self) -> usize {
        self.r
    }

    /// 校验块 = `-(H_A · m)`（GF(3) 矩阵乘），写入 `out`（长度 r）。
    pub fn parity_into(&self, message: &[Trit], out: &mut [Trit]) {
        assert_eq!(message.len(), self.k, "消息长度必须是 k");
        assert_eq!(out.len(), self.r, "校验块长度必须是 r");
        for (b, slot) in out.iter_mut().enumerate() {
            // 系统形：G = [I_k | -H_A^T]，第 b 个校验位 = -Σ m_a · H_A[a][b]。
            let mut acc = Trit::ZERO;
            for (a, &m) in message.iter().enumerate() {
                acc = add(acc, mul(m, self.cols[a][b]));
            }
            *slot = neg(acc);
        }
    }

    /// 系统码编码：`c = (m, -H_A·m)`，写入 `out`（长度 n，前 k 位是消息本身）。
    pub fn encode_into(&self, message: &[Trit], out: &mut [Trit]) {
        assert_eq!(out.len(), self.n, "码字长度必须是 n");
        out[..self.k].copy_from_slice(message);
        self.parity_into(message, &mut out[self.k..]);
    }

    /// 分配版编码。
    pub fn encode(&self, message: &[Trit]) -> Vec<Trit> {
        let mut out = vec![Trit::ZERO; self.n];
        self.encode_into(message, &mut out);
        out
    }

    /// 打包伴随式 `H·c mod 3`（索引 = Σ s_b·3^b）；`0` = 干净。
    pub fn syndrome(&self, word: &[Trit]) -> u32 {
        syndrome_index(&self.cols, self.r, word)
    }

    /// 译码并把（可能纠正过的）码字写入 `out`，返回判定。
    ///
    /// 伴随式非 0 时按单错纠正（查 [`HammingCode::syndrome`] 的表）：真单错
    /// 保证纠正；真双错会误纠成另一码字（`d=3` 界限，测试实测 100% 误纠，
    /// 但伴随式恒非零 → 永不静默通过）。
    pub fn decode_into(&self, word: &[Trit], out: &mut [Trit]) -> HammingDecode {
        assert_eq!(word.len(), self.n, "接收字长度必须是 n");
        assert_eq!(out.len(), self.n, "输出长度必须是 n");
        let idx = self.syndrome(word);
        out.copy_from_slice(word);
        match self.table[idx as usize] {
            None => HammingDecode::Clean,
            Some((pos, mag)) => {
                out[pos as usize] = sub(out[pos as usize], from_digit(mag));
                HammingDecode::Corrected {
                    position: pos as usize,
                    magnitude: mag,
                }
            }
        }
    }

    /// 分配版译码。
    pub fn decode(&self, word: &[Trit]) -> (HammingDecode, Vec<Trit>) {
        let mut out = vec![Trit::ZERO; self.n];
        let verdict = self.decode_into(word, &mut out);
        (verdict, out)
    }
}

/// 伴随式打包：`Σ_pos digit(c_pos)·列_pos mod 3`，再按 `Σ s_b·3^b` 折成索引。
fn syndrome_index(cols: &[[Trit; MAX_R]], r: usize, word: &[Trit]) -> u32 {
    (0..r)
        .map(|b| {
            let acc = word
                .iter()
                .zip(cols.iter())
                .fold(Trit::ZERO, |acc, (&c, col)| add(acc, mul(c, col[b])));
            u32::from(digit(acc)) * 3u32.pow(b as u32)
        })
        .sum()
}

/// 扩展 q-ary Hamming：整体校验位 `π = -Σ c`，d: 3 → 4。
///
/// 教科书 SECDED 的 q-ary 版本。判定规则：
/// - 伴随式 0 且总和 0 → 干净；
/// - 伴随式非 0 且表内幅度 == 全字总和 → 按单错纠正（真单错保证成立；
///   ≥3 错可能伪装成这个形态——如实界限）；
/// - 其余 → [`Suspect`](ExtendedDecode::Suspect)：判定多错，消费方应拒绝。
///
/// 双错恒不被判干净（`d=4` ⇒ 无重量 ≤3 码字）；双错里幅度互补的一半当场
/// Suspect，其余伪装成单错被误纠——帧层校验兜底，实测比率见测试注释。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtendedHamming {
    inner: HammingCode,
}

/// 扩展 Hamming 的译码判定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtendedDecode {
    /// 干净（伴随式 0 且总和 0）。
    Clean,
    /// 一致性检查通过，按单错纠正。
    Corrected { position: usize, magnitude: u8 },
    /// 判定 ≥2 错：fail-closed，消费方应拒绝整帧。
    Suspect,
}

impl ExtendedHamming {
    /// 包装一个纯 Hamming 码（块长 +1）。
    pub fn new(inner: HammingCode) -> Self {
        Self { inner }
    }

    /// 扩展块长 n+1（含整体校验位）。
    pub fn n(&self) -> usize {
        self.inner.n + 1
    }

    /// 消息长度 k。
    pub fn k(&self) -> usize {
        self.inner.k
    }

    /// 系统编码：`(m, -H_A·m, -Σ c)`，保证全字数字和 ≡ 0。
    pub fn encode_into(&self, message: &[Trit], out: &mut [Trit]) {
        assert_eq!(out.len(), self.n(), "扩展块长必须是 n+1");
        self.inner.encode_into(message, &mut out[..self.inner.n]);
        let mut sum = Trit::ZERO;
        for &t in &out[..self.inner.n] {
            sum = add(sum, t);
        }
        out[self.inner.n] = neg(sum);
    }

    /// 分配版编码。
    pub fn encode(&self, message: &[Trit]) -> Vec<Trit> {
        let mut out = vec![Trit::ZERO; self.n()];
        self.encode_into(message, &mut out);
        out
    }

    /// SECDED 译码：把（可能纠正过的）扩展块写入 `out`，返回判定。
    pub fn decode_into(&self, word: &[Trit], out: &mut [Trit]) -> ExtendedDecode {
        assert_eq!(word.len(), self.n(), "接收字长度必须是 n+1");
        assert_eq!(out.len(), self.n(), "输出长度必须是 n+1");
        out.copy_from_slice(word);
        let idx = self.inner.syndrome(&word[..self.inner.n]);
        let mut sum = Trit::ZERO;
        for &t in word {
            sum = add(sum, t);
        }
        match self.inner.table[idx as usize] {
            None if digit(sum) == 0 => ExtendedDecode::Clean,
            // s=0 且总和非 0：错误要么落在整体校验位本身（真单错，可纠正），
            // 要么是重量 ≥3 的内码码字伪装（系统码 ⇒ 消息段非零，帧层校验兜底）。
            None => {
                let mag = digit(sum);
                out[self.inner.n] = sub(out[self.inner.n], from_digit(mag));
                ExtendedDecode::Corrected {
                    position: self.inner.n,
                    magnitude: mag,
                }
            }
            Some((pos, mag)) if digit(sum) == mag => {
                out[pos as usize] = sub(out[pos as usize], from_digit(mag));
                ExtendedDecode::Corrected {
                    position: pos as usize,
                    magnitude: mag,
                }
            }
            Some(_) => ExtendedDecode::Suspect,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 确定 xorshift64*，随机测试不引依赖。
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
        fn trit(&mut self) -> Trit {
            from_digit((self.next() % 3) as u8)
        }
        fn message(&mut self, k: usize) -> Vec<Trit> {
            (0..k).map(|_| self.trit()).collect()
        }
    }

    #[test]
    fn field_ops_match_standard_digits() {
        // 3×3 全表：平衡算术结果与标准位 {0,1,2} mod 3 算术逐项一致。
        for a in 0..3u8 {
            for b in 0..3u8 {
                let ta = from_digit(a);
                let tb = from_digit(b);
                assert_eq!(digit(add(ta, tb)), (a + b) % 3, "{a}+{b}");
                assert_eq!(digit(sub(ta, tb)), (a + 3 - b) % 3, "{a}-{b}");
                assert_eq!(digit(mul(ta, tb)), (a * b) % 3, "{a}*{b}");
            }
        }
        assert_eq!(digit(neg(from_digit(2))), 1);
        // 平衡环回的直观例子：(-1)+(-1) = -2 ≡ +1（不是饱和截断）。
        assert_eq!(add(Trit::NEG, Trit::NEG), Trit::POS);
        assert_eq!(add(Trit::POS, Trit::POS), Trit::NEG);
    }

    #[test]
    fn digit_conversions_roundtrip() {
        for d in 0..3u8 {
            assert_eq!(digit(from_digit(d)), d);
        }
        for t in [Trit::NEG, Trit::ZERO, Trit::POS] {
            assert_eq!(from_digit(digit(t)), t);
        }
    }

    #[test]
    fn r3_full_codeword_space_single_error() {
        // 交付口径：r=3 全码字空间 3^10 = 59049 个码字 × 单 trit 错
        // （每位置 3 种取值：1 种不动 + 2 种真错 = 39 种）全部枚举。
        let code = HammingCode::r3();
        let mut cases = 0u64;
        let mut clean = 0u64;
        for raw in 0..3u32.pow(10) {
            let message: Vec<Trit> = (0..10)
                .map(|i| from_digit(((raw / 3u32.pow(i as u32)) % 3) as u8))
                .collect();
            let word = code.encode(&message);
            assert_eq!(code.syndrome(&word), 0, "码字伴随式必须为 0");
            for pos in 0..code.n() {
                for mag in 1..=2u8 {
                    let mut bad = word.clone();
                    bad[pos] = add(bad[pos], from_digit(mag));
                    let (verdict, fixed) = code.decode(&bad);
                    assert_eq!(
                        verdict,
                        HammingDecode::Corrected {
                            position: pos,
                            magnitude: mag,
                        },
                        "消息 {raw} 位置 {pos} 幅度 {mag}"
                    );
                    assert_eq!(fixed, word, "必须纠正回原码字");
                    cases += 1;
                }
                // 不动的情形必须判干净。
                let (verdict, _) = code.decode(&word);
                assert!(matches!(verdict, HammingDecode::Clean));
                clean += 1;
            }
        }
        assert_eq!(cases, 59049 * 26);
        assert_eq!(clean, 59049 * 13);
    }

    #[test]
    fn r3_double_error_always_flagged_and_miscorrected() {
        // 双错的如实界限：伴随式恒非零（检出），但译码必误纠成另一码字。
        // 伴随式只依赖 (位置对, 幅度对) 与 H——先在 H 层面全枚举 312 种组合，
        // 再抽样码字验证消息级后果。
        let code = HammingCode::r3();
        let mut h_combos = 0u32;
        for i in 0..code.n() {
            for j in (i + 1)..code.n() {
                for a in 1..=2u8 {
                    for b in 1..=2u8 {
                        let mut e = vec![Trit::ZERO; code.n()];
                        e[i] = from_digit(a);
                        e[j] = from_digit(b);
                        assert_ne!(
                            code.syndrome(&e),
                            0,
                            "列对 ({i},{j}) 幅度 ({a},{b}) 伴随式为 0 ⇒ 存在重量 2 码字"
                        );
                        h_combos += 1;
                    }
                }
            }
        }
        assert_eq!(h_combos, 312);
        // 抽样 729 个码字 × 全部 312 种双错：全部检出 + 全部误纠（消息必变）。
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let mut flagged = 0u64;
        let mut miscorrected = 0u64;
        for _ in 0..729 {
            let message = rng.message(code.k());
            let word = code.encode(&message);
            for i in 0..code.n() {
                for j in (i + 1)..code.n() {
                    for (a, b) in [(1u8, 1u8), (1, 2), (2, 1), (2, 2)] {
                        let mut bad = word.clone();
                        bad[i] = add(bad[i], from_digit(a));
                        bad[j] = add(bad[j], from_digit(b));
                        let (verdict, fixed) = code.decode(&bad);
                        assert!(
                            matches!(verdict, HammingDecode::Corrected { .. }),
                            "双错伴随式非零，必被（误）纠正而不是判干净"
                        );
                        flagged += 1;
                        if fixed[..code.k()] != message[..] {
                            miscorrected += 1;
                        }
                    }
                }
            }
        }
        // 如实口径：系统码下双错误纠 100% 改变消息（e−f 是非零系统码字，
        // 消息段不可能全零），但状态永远是"被纠正"而非"干净"。
        assert_eq!(flagged, 729 * 312);
        assert_eq!(miscorrected, flagged, "双错 100% 误纠到别的消息");
    }

    #[test]
    fn r4_projective_completeness_and_sampled_single_error() {
        let code = HammingCode::r4();
        assert_eq!((code.n(), code.k(), code.r()), (40, 36, 4));
        // 射影完备性：3^4 - 1 = 80 个非零向量，每条射线恰好命中一列。
        for raw in 1..81u32 {
            let v: Vec<u8> = (0..4)
                .map(|i| ((raw / 3u32.pow(i as u32)) % 3) as u8)
                .collect();
            let hits = (0..code.n())
                .filter(|&p| {
                    let col = code.cols[p];
                    // v 与列共线 ⟺ 逐坐标 digit(v_i)·digit(col_j) ≡ digit(v_j)·digit(col_i)。
                    (0..4).all(|i| {
                        (0..4).all(|j| {
                            u16::from(digit(col[i])) * u16::from(v[j]) % 3
                                == u16::from(digit(col[j])) * u16::from(v[i]) % 3
                        })
                    })
                })
                .count();
            assert_eq!(hits, 1, "向量 {v:?} 命中 {hits} 条射线");
        }
        // 抽样单错：729 个随机码字 × 80 种单错型。
        let mut rng = Rng(0x0123_4567_89AB_CDEF);
        for _ in 0..729 {
            let message = rng.message(code.k());
            let word = code.encode(&message);
            for pos in 0..code.n() {
                for mag in 1..=2u8 {
                    let mut bad = word.clone();
                    bad[pos] = add(bad[pos], from_digit(mag));
                    let (verdict, fixed) = code.decode(&bad);
                    assert_eq!(
                        verdict,
                        HammingDecode::Corrected {
                            position: pos,
                            magnitude: mag
                        }
                    );
                    assert_eq!(fixed, word);
                }
            }
        }
    }

    #[test]
    fn random_roundtrips_all_clean() {
        // 交付口径：随机 10k 码字 roundtrip（r=3 / r=4 各一份）。
        for (r, seed) in [(3u32, 0xDEAD_BEEF_1234_5678u64), (4, 0x0F0F_0F0F_AAAA_5555)] {
            let code = HammingCode::new(r).expect("r 支持");
            let mut rng = Rng(seed);
            for _ in 0..10_000 {
                let message = rng.message(code.k());
                let word = code.encode(&message);
                let (verdict, fixed) = code.decode(&word);
                assert_eq!(verdict, HammingDecode::Clean);
                assert_eq!(fixed, word);
            }
        }
    }

    #[test]
    fn extended_hamming_secdec_semantics() {
        // 扩展 [14,10]：单错全数纠正（抽样），双错全数 Suspect 或误纠（绝不判干净），
        // Suspect 比率是构造相关常数——实测钉死。
        let ext = ExtendedHamming::new(HammingCode::r3());
        let mut rng = Rng(0xC0FF_EE12_3456_789A);
        let mut corrected = 0u64;
        for _ in 0..4096 {
            let message = rng.message(ext.k());
            let word = ext.encode(&message);
            assert_eq!(
                word.iter().map(|&t| digit(t)).sum::<u8>() % 3,
                0,
                "扩展码字总和为 0"
            );
            for pos in 0..ext.n() {
                for mag in 1..=2u8 {
                    let mut bad = word.clone();
                    bad[pos] = add(bad[pos], from_digit(mag));
                    let mut fixed = vec![Trit::ZERO; ext.n()];
                    let verdict = ext.decode_into(&bad, &mut fixed);
                    assert_eq!(
                        verdict,
                        ExtendedDecode::Corrected {
                            position: pos,
                            magnitude: mag
                        },
                        "单错必须被一致性检查识别"
                    );
                    assert_eq!(fixed, word);
                    corrected += 1;
                }
            }
        }
        assert_eq!(corrected, 4096 * 28);
        // 全部 312 种双错型 × 抽样码字：永不 Clean；统计 Suspect 比率。
        let mut suspect = 0u64;
        let mut miscorrected = 0u64;
        for _ in 0..64 {
            let message = rng.message(ext.k());
            let word = ext.encode(&message);
            for i in 0..ext.inner.n {
                for j in (i + 1)..ext.inner.n {
                    for (a, b) in [(1u8, 1u8), (1, 2), (2, 1), (2, 2)] {
                        let mut bad = word.clone();
                        bad[i] = add(bad[i], from_digit(a));
                        bad[j] = add(bad[j], from_digit(b));
                        let mut fixed = vec![Trit::ZERO; ext.n()];
                        match ext.decode_into(&bad, &mut fixed) {
                            ExtendedDecode::Suspect => suspect += 1,
                            ExtendedDecode::Corrected { .. } => {
                                // 误纠：消息必变（e−f 非零系统码字）。
                                miscorrected += 1;
                                assert_ne!(fixed[..ext.k()], message[..]);
                            }
                            ExtendedDecode::Clean => {
                                panic!("d=4：双错不可能判干净");
                            }
                        }
                    }
                }
            }
        }
        let total = suspect + miscorrected;
        // 实测：约 69% 的双错被一致性检查当场拒绝，其余伪装成单错被误纠
        // （由列规范形的首非零位置结构决定，测试钉死防回归）。
        // 64 样本 × 312 = 19968 例。
        assert_eq!(total, 64 * 312);
        let ratio_bp = suspect * 10_000 / total;
        assert!(ratio_bp >= 6_000, "Suspect 比率 {ratio_bp}bp，应 ≥ 60%");
        assert!(ratio_bp <= 8_000, "Suspect 比率 {ratio_bp}bp，应 ≤ 80%");
    }
}
