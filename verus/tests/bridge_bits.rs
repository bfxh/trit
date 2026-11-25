//! `TriBit`（位平面）与标量 `Ternary` 的穷举对拍。
//!
//! 位平面是 64 位并行的；这里对 8 组边界/交错平面 × 64 个位位置 × 3 种代数
//! 逐位比较，覆盖 `not` / `and` / `or`。
//! Verus 侧目前证明 `not` 的逐位等价；`and` / `or` 的位向量证明见 README 的缺口。

use trit_core::algebra::Algebra;
use trit_core::bits::{BitPlane, TriBit};

const PLANES: [u64; 8] = [
    0,
    u64::MAX,
    0xAAAA_AAAA_AAAA_AAAA,
    0x5555_5555_5555_5555,
    1,
    1 << 63,
    0x0F0F_0F0F_0F0F_0F0F,
    0xF0F0_F0F0_F0F0_F0F0,
];

fn plane(ones: u64, unknowns: u64) -> TriBit {
    TriBit {
        ones: BitPlane(ones),
        unknowns: BitPlane(unknowns),
    }
}

#[test]
fn tri_bit_not_matches_scalar() {
    for &ones in &PLANES {
        for &unknowns in &PLANES {
            let value = plane(ones, unknowns);
            let negated = value.not();
            for bit in 0..64 {
                assert_eq!(negated.get(bit), !value.get(bit), "not bit {bit}");
            }
        }
    }
}

#[test]
fn tri_bit_and_matches_scalar() {
    for &alg in &Algebra::ALL {
        for &o1 in &PLANES {
            for &u1 in &PLANES {
                for &o2 in &PLANES {
                    for &u2 in &PLANES {
                        let left = plane(o1, u1);
                        let right = plane(o2, u2);
                        let result = left.and(right, alg);
                        for bit in 0..64 {
                            assert_eq!(
                                result.get(bit),
                                alg.and(left.get(bit), right.get(bit)),
                                "{alg} and bit {bit}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn tri_bit_or_matches_scalar() {
    for &alg in &Algebra::ALL {
        for &o1 in &PLANES {
            for &u1 in &PLANES {
                for &o2 in &PLANES {
                    for &u2 in &PLANES {
                        let left = plane(o1, u1);
                        let right = plane(o2, u2);
                        let result = left.or(right, alg);
                        for bit in 0..64 {
                            assert_eq!(
                                result.get(bit),
                                alg.or(left.get(bit), right.get(bit)),
                                "{alg} or bit {bit}"
                            );
                        }
                    }
                }
            }
        }
    }
}
