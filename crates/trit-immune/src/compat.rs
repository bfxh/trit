//! trit-core 互操作：[`Trit`] ↔ [`Ternary`] 双向映射。
//!
//! 两个类型**完全同构**（映射 `Vulnerable↔False`、`Unknown↔Unknown`、
//! `Hardened↔True` 下，K3 AND ≡ Kleene BitAnd——False/Vulnerable 都是
//! 支配元，Ord 都是坏→好）：本模块提供双向 `From` 与穷举同构测试
//! （3×3 全空间对拍 [`Trit::and`] 与 [`Ternary::bitand`]）。
//!
//! 设计立场（PR.md）：trit-immune 是 trit-core `Ternary` 的**消费者**，
//! 不是竞品——内核逻辑用本地 `Trit` 保持零依赖，边界处经本模块与
//! trit-core 生态互换值。

use crate::trit::Trit;
use trit_core::Ternary;

impl From<Trit> for Ternary {
    fn from(t: Trit) -> Self {
        match t {
            Trit::Vulnerable => Ternary::False,
            Trit::Unknown => Ternary::Unknown,
            Trit::Hardened => Ternary::True,
        }
    }
}

impl From<Ternary> for Trit {
    fn from(t: Ternary) -> Self {
        match t {
            Ternary::False => Trit::Vulnerable,
            Ternary::Unknown => Trit::Unknown,
            Ternary::True => Trit::Hardened,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_is_identity() {
        for t in [Trit::Vulnerable, Trit::Unknown, Trit::Hardened] {
            assert_eq!(Trit::from(Ternary::from(t)), t, "双向往返恒等");
        }
    }

    /// K3 AND ≡ Kleene BitAnd 在映射下同构（3×3 全空间穷举）。
    #[test]
    fn and_is_homomorphic_to_bitand() {
        let trits = [Trit::Vulnerable, Trit::Unknown, Trit::Hardened];
        let terns = [Ternary::False, Ternary::Unknown, Ternary::True];
        for (&a, &ta) in trits.iter().zip(terns.iter()) {
            for (&b, &tb) in trits.iter().zip(terns.iter()) {
                assert_eq!(
                    Ternary::from(a.and(b)),
                    ta & tb,
                    "{a:?}∧{b:?} 应与 Ternary bitand 同构"
                );
            }
        }
    }
}
