//! K3 三值语义的 Verus 规格与证明。
//!
//! 这里的 `spec fn` 是语义定义；`table.rs` 是同一语义的编码表。
//! `table_*_matches_spec` 证明两者一致；`tests/bridge.rs` 再用普通 Rust
//! 把编码表与 `trit-core::value::Ternary` 的 `&`/`|`/`!`/`^` 逐项对拍。

use vstd::prelude::*;

verus! {

/// K3（强 Kleene）三值：F < U < T，U 是未知。
#[derive(PartialEq, Eq)]
pub enum Trit {
    F,
    U,
    T,
}

pub open spec fn knot(t: Trit) -> Trit {
    match t {
        Trit::F => Trit::T,
        Trit::U => Trit::U,
        Trit::T => Trit::F,
    }
}

pub open spec fn tand(a: Trit, b: Trit) -> Trit {
    match (a, b) {
        (Trit::F, _) => Trit::F,
        (_, Trit::F) => Trit::F,
        (Trit::T, Trit::T) => Trit::T,
        _ => Trit::U,
    }
}

pub open spec fn tor(a: Trit, b: Trit) -> Trit {
    match (a, b) {
        (Trit::T, _) => Trit::T,
        (_, Trit::T) => Trit::T,
        (Trit::F, Trit::F) => Trit::F,
        _ => Trit::U,
    }
}

pub open spec fn teq(a: Trit, b: Trit) -> Trit {
    match (a, b) {
        (Trit::U, _) => Trit::U,
        (_, Trit::U) => Trit::U,
        (Trit::T, Trit::T) => Trit::T,
        (Trit::F, Trit::F) => Trit::T,
        _ => Trit::F,
    }
}

pub open spec fn tne(a: Trit, b: Trit) -> Trit {
    knot(teq(a, b))
}

pub open spec fn rank(t: Trit) -> int {
    match t {
        Trit::F => 0,
        Trit::U => 1,
        Trit::T => 2,
    }
}

/// 双重否定。
pub proof fn knot_involution(t: Trit)
    ensures knot(knot(t)) == t,
{
    match t {
        Trit::F => {},
        Trit::U => {},
        Trit::T => {},
    }
}

/// K3 合取交换律。
pub proof fn and_commutative(a: Trit, b: Trit)
    ensures tand(a, b) == tand(b, a),
{
    match a {
        Trit::F => match b {
            Trit::F => {},
            Trit::U => {},
            Trit::T => {},
        },
        Trit::U => match b {
            Trit::F => {},
            Trit::U => {},
            Trit::T => {},
        },
        Trit::T => match b {
            Trit::F => {},
            Trit::U => {},
            Trit::T => {},
        },
    }
}

/// K3 析取交换律。
pub proof fn or_commutative(a: Trit, b: Trit)
    ensures tor(a, b) == tor(b, a),
{
    match a {
        Trit::F => match b {
            Trit::F => {},
            Trit::U => {},
            Trit::T => {},
        },
        Trit::U => match b {
            Trit::F => {},
            Trit::U => {},
            Trit::T => {},
        },
        Trit::T => match b {
            Trit::F => {},
            Trit::U => {},
            Trit::T => {},
        },
    }
}

/// De Morgan（K3 是 De Morgan 代数）。
pub proof fn de_morgan_and(a: Trit, b: Trit)
    ensures knot(tand(a, b)) == tor(knot(a), knot(b)),
{
    match a {
        Trit::F => match b {
            Trit::F => {},
            Trit::U => {},
            Trit::T => {},
        },
        Trit::U => match b {
            Trit::F => {},
            Trit::U => {},
            Trit::T => {},
        },
        Trit::T => match b {
            Trit::F => {},
            Trit::U => {},
            Trit::T => {},
        },
    }
}

pub proof fn de_morgan_or(a: Trit, b: Trit)
    ensures knot(tor(a, b)) == tand(knot(a), knot(b)),
{
    match a {
        Trit::F => match b {
            Trit::F => {},
            Trit::U => {},
            Trit::T => {},
        },
        Trit::U => match b {
            Trit::F => {},
            Trit::U => {},
            Trit::T => {},
        },
        Trit::T => match b {
            Trit::F => {},
            Trit::U => {},
            Trit::T => {},
        },
    }
}

/// 已知值与自身的 K3 相等为 T；未知值恒为 U。
pub proof fn teq_reflexive(t: Trit)
    ensures
        t != Trit::U ==> teq(t, t) == Trit::T,
        t == Trit::U ==> teq(t, t) == Trit::U,
{
    match t {
        Trit::F => {},
        Trit::U => {},
        Trit::T => {},
    }
}

/// 相等与不相等互补。
pub proof fn teq_tne_complement(a: Trit, b: Trit)
    ensures tne(a, b) == knot(teq(a, b)),
{
}

/// rank 是单射：不同三值 rank 不同。
pub proof fn rank_injective(a: Trit, b: Trit)
    ensures rank(a) == rank(b) ==> a == b,
{
    match a {
        Trit::F => match b {
            Trit::F => {},
            Trit::U => {},
            Trit::T => {},
        },
        Trit::U => match b {
            Trit::F => {},
            Trit::U => {},
            Trit::T => {},
        },
        Trit::T => match b {
            Trit::F => {},
            Trit::U => {},
            Trit::T => {},
        },
    }
}

// ---- exec 提炼：证明可执行版本与 spec 一致 -------------------------------------

pub fn knot_exec(t: Trit) -> (r: Trit)
    ensures r == knot(t),
{
    match t {
        Trit::F => Trit::T,
        Trit::U => Trit::U,
        Trit::T => Trit::F,
    }
}

pub fn tand_exec(a: Trit, b: Trit) -> (r: Trit)
    ensures r == tand(a, b),
{
    match (a, b) {
        (Trit::F, _) => Trit::F,
        (_, Trit::F) => Trit::F,
        (Trit::T, Trit::T) => Trit::T,
        _ => Trit::U,
    }
}

pub fn tor_exec(a: Trit, b: Trit) -> (r: Trit)
    ensures r == tor(a, b),
{
    match (a, b) {
        (Trit::T, _) => Trit::T,
        (_, Trit::T) => Trit::T,
        (Trit::F, Trit::F) => Trit::F,
        _ => Trit::U,
    }
}

pub fn teq_exec(a: Trit, b: Trit) -> (r: Trit)
    ensures r == teq(a, b),
{
    match (a, b) {
        (Trit::U, _) => Trit::U,
        (_, Trit::U) => Trit::U,
        (Trit::T, Trit::T) => Trit::T,
        (Trit::F, Trit::F) => Trit::T,
        _ => Trit::F,
    }
}

} // verus!
