//! 三元 Golay `[11,6,5]`：GF(3) 上的完美码。
//!
//! 教科书构造（出处只给名字：Paley 二次剩余构造 mod 11；MacWilliams &
//! Sloane, *The Theory of Error-Correcting Codes*；Conway & Sloane,
//! *Sphere Packings, Lattices and Groups*）：Paley Hadamard `H_12`
//! （`H·Hᵀ = 12·I`，11 ≡ 0 无平方剩余）在 GF(3) 上行空间自正交、秩 6，
//! 是扩展三元 Golay `[12,6,6]`（重量分布 {0:1, 6:264, 9:440, 12:24}，
//! 全部 ≡ 0 mod 3）；任意打洞一列得 `[11,6,5]`。本模块嵌入其系统形
//! `G = [I₆ | B]`（对齐基线由 QR 构造约简导出，属性由测试钉死）。
//!
//! # 参数与界限
//!
//! `n=11, k=6, d=5`：**保证纠正 ≤2 个 trit 错**。完美码 ⇒ 1 + 2·11 +
//! 4·C(11,2) = 243 = 3⁵ 个重量 ≤2 错型与伴随式**双射**（[`Golay11`]
//! 构造时建立查表）；因此译码可**确定区分** 1 错与 2 错（`d≥5` ⇒ 二错
//! 伴随式永不与单错碰撞）。≥3 错会被伪装成某 ≤2 错码型误纠（完美码的固有
//! 界限，如实写：伴随式永远有 ≤2 重的 leader 可查）——帧层
//! （[`crate::frame`]）用"纠 1、拒 2、校验兜底"的消费策略处理。

use crate::gf3::{add, digit, from_digit, mul, neg, sub};
use crate::trit::Trit;

/// 码字长度。
pub const N: usize = 11;
/// 消息长度。
pub const K: usize = 6;
/// 最小汉明距离。
pub const DISTANCE: usize = 5;
/// 伴随式状态数 = 3⁵ = 243（完美码：243 个重量 ≤2 错型双射覆盖）。
const SYNDROMES: usize = 243;

/// 生成矩阵系统形右块 `B`（6×5，标准位 {0,1,2}）：`G = [I₆ | B]`。
///
/// 由 Paley `H_12` 行空间（[12,6,6]）RREF 系统化后打洞末列导出；
/// d=5、729 码字、243 伴随式双射由测试验证。
const B: [[u8; 5]; 6] = [
    [2, 2, 1, 2, 0],
    [0, 2, 2, 1, 2],
    [2, 2, 0, 1, 1],
    [1, 0, 1, 1, 1],
    [1, 2, 2, 2, 1],
    [1, 2, 1, 0, 2],
];

/// 一次 Golay 译码结果（纠正过的码字写在 `decode_into` 的输出缓冲里）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GolayDecode {
    /// 伴随式 0（≥5 错可能伪装成干净，d=5 的固有界限）。
    Clean,
    /// 确定 1 错：已按 `(位置, 幅度标准位)` 纠正。
    Corrected1 { position: usize, magnitude: u8 },
    /// 确定 2 错：已按 `[(位置, 幅度); 2]`（位置升序）纠正。
    ///
    /// 完美码语义下这是**可靠判定**（`d≥5` ⇒ 双错伴随式不与单错碰撞）；
    /// 消费方（frame 层）按 fail-closed 策略拒绝双错帧。
    Corrected2 { errors: [(usize, u8); 2] },
}

/// 伴随式对应的错误样式（coset leader）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Leader {
    None,
    One(u8, u8),
    Two(u8, u8, u8, u8),
}

/// 三元 Golay `[11,6,5]`。构造建立 243 项伴随式查表（重量 ≤2 错型双射）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Golay11 {
    /// 系统校验块（G 的右块，平衡 trit）。
    parity: [[Trit; 5]; 6],
    /// 校验矩阵行 `H = [-Bᵀ | I₅]`（5 行 × 11 列）。
    h_rows: [[Trit; N]; 5],
    /// 243 项伴随式 → 错型查表。
    table: [Leader; SYNDROMES],
}

impl Default for Golay11 {
    fn default() -> Self {
        Self::new()
    }
}

/// 单错向量（标准位幅度 → 平衡 trit）。
fn error_at(pos: usize, mag: u8) -> [Trit; N] {
    let mut e = [Trit::ZERO; N];
    e[pos] = from_digit(mag);
    e
}

impl Golay11 {
    /// 构造码并建立伴随式表。
    pub fn new() -> Self {
        let parity: [[Trit; 5]; 6] = B.map(|row| {
            let mut p = [Trit::ZERO; 5];
            for (j, &d) in row.iter().enumerate() {
                p[j] = from_digit(d);
            }
            p
        });
        let mut h_rows = [[Trit::ZERO; N]; 5];
        for t in 0..5 {
            for i in 0..K {
                h_rows[t][i] = neg(parity[i][t]);
            }
            h_rows[t][K + t] = Trit::POS;
        }
        // 查表：零错 + 全部 1 错（22）+ 全部 2 错（220）→ 243 个互异伴随式。
        let mut table = [Leader::None; SYNDROMES];
        let mut filled = 1usize;
        for pos in 0..N {
            for mag in 1..=2u8 {
                let idx = syndrome_of(&h_rows, &error_at(pos, mag));
                debug_assert!(
                    matches!(table[idx as usize], Leader::None),
                    "1 错伴随式碰撞"
                );
                table[idx as usize] = Leader::One(pos as u8, mag);
                filled += 1;
            }
        }
        for p in 0..N {
            for q in (p + 1)..N {
                for a in 1..=2u8 {
                    for b in 1..=2u8 {
                        let mut e = error_at(p, a);
                        e[q] = from_digit(b);
                        let idx = syndrome_of(&h_rows, &e);
                        debug_assert!(
                            matches!(table[idx as usize], Leader::None),
                            "2 错伴随式碰撞"
                        );
                        table[idx as usize] = Leader::Two(p as u8, a, q as u8, b);
                        filled += 1;
                    }
                }
            }
        }
        debug_assert_eq!(filled, SYNDROMES, "完美码：243 个伴随式全覆盖");
        Self {
            parity,
            h_rows,
            table,
        }
    }

    /// 码字长度 11。
    pub fn n(&self) -> usize {
        N
    }

    /// 消息长度 6。
    pub fn k(&self) -> usize {
        K
    }

    /// 系统编码：`c = (m, m·B)`，写入 `out`（长度 11，前 6 位是消息本身）。
    pub fn encode_into(&self, message: &[Trit], out: &mut [Trit]) {
        assert_eq!(message.len(), K, "消息长度必须是 6");
        assert_eq!(out.len(), N, "码字长度必须是 11");
        out[..K].copy_from_slice(message);
        for (j, slot) in out[K..].iter_mut().enumerate() {
            let mut acc = Trit::ZERO;
            for (i, &m) in message.iter().enumerate() {
                acc = add(acc, mul(m, self.parity[i][j]));
            }
            *slot = acc;
        }
    }

    /// 分配版编码。
    pub fn encode(&self, message: &[Trit]) -> Vec<Trit> {
        let mut out = vec![Trit::ZERO; N];
        self.encode_into(message, &mut out);
        out
    }

    /// 打包伴随式；`0` = 干净。
    pub fn syndrome(&self, word: &[Trit]) -> u32 {
        assert_eq!(word.len(), N, "接收字长度必须是 11");
        let mut w = [Trit::ZERO; N];
        w.copy_from_slice(word);
        syndrome_of(&self.h_rows, &w)
    }

    /// 译码：把（可能纠正过的）码字写入 `out`，返回判定。
    ///
    /// 243 个伴随式各恰有一个重量 ≤2 的 leader：≤2 错保证纠正且**确定
    /// 区分 1 错 / 2 错**；≥3 错伪装成某 ≤2 错型被误纠（完美码界限）。
    pub fn decode_into(&self, word: &[Trit], out: &mut [Trit]) -> GolayDecode {
        assert_eq!(word.len(), N, "接收字长度必须是 11");
        assert_eq!(out.len(), N, "输出长度必须是 11");
        let idx = self.syndrome(word);
        out.copy_from_slice(word);
        match self.table[idx as usize] {
            Leader::None => GolayDecode::Clean,
            Leader::One(p, m) => {
                out[p as usize] = sub(out[p as usize], from_digit(m));
                GolayDecode::Corrected1 {
                    position: p as usize,
                    magnitude: m,
                }
            }
            Leader::Two(p, a, q, b) => {
                out[p as usize] = sub(out[p as usize], from_digit(a));
                out[q as usize] = sub(out[q as usize], from_digit(b));
                GolayDecode::Corrected2 {
                    errors: [(p as usize, a), (q as usize, b)],
                }
            }
        }
    }

    /// 分配版译码。
    pub fn decode(&self, word: &[Trit]) -> (GolayDecode, Vec<Trit>) {
        let mut out = vec![Trit::ZERO; N];
        let verdict = self.decode_into(word, &mut out);
        (verdict, out)
    }
}

/// 伴随式打包：`s = H·cᵀ`，索引 `Σ digit(s_t)·3^t`。
fn syndrome_of(h_rows: &[[Trit; N]; 5], word: &[Trit; N]) -> u32 {
    let mut idx = 0u32;
    for (t, row) in h_rows.iter().enumerate() {
        let mut acc = Trit::ZERO;
        for (pos, &w) in word.iter().enumerate() {
            acc = add(acc, mul(w, row[pos]));
        }
        idx += u32::from(digit(acc)) * 3u32.pow(t as u32);
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 确定 xorshift64*（随机测试不引依赖）。
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
        fn message(&mut self) -> Vec<Trit> {
            (0..K)
                .map(|_| from_digit((self.next() % 3) as u8))
                .collect()
        }
    }

    #[test]
    fn all_codewords_have_zero_syndrome_and_weight_stats() {
        // 729 个码字全枚举：伴随式全 0；重量分布钉死（d=5 + 完美码的指纹）。
        let code = Golay11::new();
        let mut weights = [0usize; N + 1];
        for raw in 0..3u32.pow(K as u32) {
            let message: Vec<Trit> = (0..K)
                .map(|i| from_digit(((raw / 3u32.pow(i as u32)) % 3) as u8))
                .collect();
            let word = code.encode(&message);
            assert_eq!(code.syndrome(&word), 0, "消息 {raw}");
            let w = word.iter().filter(|&&t| t != Trit::ZERO).count();
            weights[w] += 1;
        }
        // 对齐基线（Paley 导出）实测：{5:132, 6:132, 8:330, 9:110, 11:24}。
        assert_eq!(weights, [1, 0, 0, 0, 0, 132, 132, 0, 330, 110, 0, 24]);
    }

    #[test]
    fn full_space_up_to_two_errors_truth_table() {
        // 交付口径：全码字空间 3^6 = 729 × 全部 ≤2 错型（1+22+220 = 243）
        // = 177147 例，全部纠正回原码字且判定精确（1 错 / 2 错确定性区分）。
        let code = Golay11::new();
        let mut cases = 0u64;
        for raw in 0..3u32.pow(K as u32) {
            let message: Vec<Trit> = (0..K)
                .map(|i| from_digit(((raw / 3u32.pow(i as u32)) % 3) as u8))
                .collect();
            let word = code.encode(&message);
            let (verdict, fixed) = code.decode(&word);
            assert_eq!(verdict, GolayDecode::Clean);
            assert_eq!(fixed, word);
            cases += 1;
            for pos in 0..N {
                for mag in 1..=2u8 {
                    let mut bad = word.clone();
                    bad[pos] = add(bad[pos], from_digit(mag));
                    let (verdict, fixed) = code.decode(&bad);
                    assert_eq!(
                        verdict,
                        GolayDecode::Corrected1 {
                            position: pos,
                            magnitude: mag,
                        },
                        "消息 {raw} 位置 {pos} 幅度 {mag}"
                    );
                    assert_eq!(fixed, word);
                    cases += 1;
                }
            }
            for p in 0..N {
                for q in (p + 1)..N {
                    for a in 1..=2u8 {
                        for b in 1..=2u8 {
                            let mut bad = word.clone();
                            bad[p] = add(bad[p], from_digit(a));
                            bad[q] = add(bad[q], from_digit(b));
                            let (verdict, fixed) = code.decode(&bad);
                            assert_eq!(
                                verdict,
                                GolayDecode::Corrected2 {
                                    errors: [(p, a), (q, b)],
                                },
                                "消息 {raw} 错型 ({p},{a})+({q},{b})"
                            );
                            assert_eq!(fixed, word);
                            cases += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(cases, 729 * 243);
    }

    #[test]
    fn random_roundtrips_all_clean() {
        // 交付口径：随机 10k 码字 roundtrip。
        let code = Golay11::new();
        let mut rng = Rng(0x5EED_5EED_5EED_0001);
        for _ in 0..10_000 {
            let message = rng.message();
            let word = code.encode(&message);
            let (verdict, fixed) = code.decode(&word);
            assert_eq!(verdict, GolayDecode::Clean);
            assert_eq!(fixed, word);
        }
    }

    #[test]
    fn three_errors_always_masquerade_as_repairable_never_as_clean() {
        // 如实界限：完美码 ⇒ 每个 3 错字的伴随式都命中某 ≤2 错 leader，
        // 即全部被误纠（"纠 2"以上的错不被拒绝而是落到别的码字）；
        // 判 Clean 需要 3 错本身是码字——d=5 排除。抽样枚举钉死。
        let code = Golay11::new();
        let mut rng = Rng(0xABCD_EF01_2345_6789);
        let mut miscorrected = 0u64;
        for _ in 0..8 {
            let word = code.encode(&rng.message());
            for p in 0..N {
                for q in (p + 1)..N {
                    for r in (q + 1)..N {
                        for (a, b, c) in
                            [(1u8, 1u8, 1u8), (1u8, 1u8, 2), (1u8, 2u8, 2), (2u8, 2u8, 2)]
                        {
                            let mut bad = word.clone();
                            bad[p] = add(bad[p], from_digit(a));
                            bad[q] = add(bad[q], from_digit(b));
                            bad[r] = add(bad[r], from_digit(c));
                            let (verdict, fixed) = code.decode(&bad);
                            assert!(
                                !matches!(verdict, GolayDecode::Clean),
                                "d=5：3 错不可能判干净"
                            );
                            assert_ne!(fixed, word, "误纠必落到别的码字");
                            assert_eq!(code.syndrome(&fixed), 0, "误纠结果仍是合法码字");
                            miscorrected += 1;
                        }
                    }
                }
            }
        }
        // 8 样本 × C(11,3) × 4 = 8 × 165 × 4 = 5280 例，全部误纠。
        assert_eq!(miscorrected, 8 * 165 * 4);
    }
}
