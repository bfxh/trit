//! 主库 `crates/trit-core/src/value.rs` 的 K3 算子实现体。
//!
//! `// BEGIN K3 OPS` / `// END K3 OPS` 之间的函数体由
//! `scripts/check_k3_impl.py` 与主库源码逐字比对（规范化 `Self::*` / `self` / `other`
//! 后）。这样 Verus 验证的就是主库实际在跑的同一段实现，而不是另一份重写。

use vstd::prelude::*;

use crate::k3::Trit;

#[cfg(verus_only)]
use crate::k3::{knot, tand, tor};

verus! {

/// 经典异或：任一 `U` 即 `U`；两个已知值相同为 `F`，不同为 `T`。
pub open spec fn txor(a: Trit, b: Trit) -> Trit {
    match (a, b) {
        (Trit::U, _) => Trit::U,
        (_, Trit::U) => Trit::U,
        (Trit::T, Trit::T) => Trit::F,
        (Trit::F, Trit::F) => Trit::F,
        _ => Trit::T,
    }
}

// BEGIN K3 OPS
pub fn not_impl(a: Trit) -> (r: Trit)
    ensures r == knot(a),
{
    match a {
        Trit::F => Trit::T,
        Trit::T => Trit::F,
        Trit::U => Trit::U,
    }
}

pub fn bitand_impl(a: Trit, b: Trit) -> (r: Trit)
    ensures r == tand(a, b),
{
    match (a, b) {
        (Trit::F, _) | (_, Trit::F) => Trit::F,
        (Trit::T, Trit::T) => Trit::T,
        _ => Trit::U,
    }
}

pub fn bitor_impl(a: Trit, b: Trit) -> (r: Trit)
    ensures r == tor(a, b),
{
    match (a, b) {
        (Trit::T, _) | (_, Trit::T) => Trit::T,
        (Trit::F, Trit::F) => Trit::F,
        _ => Trit::U,
    }
}

pub fn bitxor_impl(a: Trit, b: Trit) -> (r: Trit)
    ensures r == txor(a, b),
{
    match (a, b) {
        (Trit::U, _) | (_, Trit::U) => Trit::U,
        (Trit::T, Trit::T) | (Trit::F, Trit::F) => Trit::F,
        _ => Trit::T,
    }
}
// END K3 OPS

}
