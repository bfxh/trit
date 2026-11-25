//! Łukasiewicz 三值（Ł3）的 Verus 规格与证明。
//!
//! 值域用 `int` 的双倍刻度表示：`F = 0`、`U = 1`、`T = 2`（即 0 / 0.5 / 1）。
//! 这样 `min(1, a+b)`、`max(0, a+b-1)`、`min(1, 1-a+b)` 都能用整数精确表达，
//! 避免浮点在证明里出现。

use vstd::prelude::*;

verus! {

/// Ł3 三值：F < U < T。
#[derive(PartialEq, Eq, Clone, Copy)]
pub enum Trit {
    F,
    U,
    T,
}

/// 双倍刻度：0 / 1 / 2。
pub open spec fn val(t: Trit) -> int {
    match t {
        Trit::F => 0,
        Trit::U => 1,
        Trit::T => 2,
    }
}

/// 由双倍刻度还原；0 以下取 F，2 以上取 T。
pub open spec fn of_val(v: int) -> Trit {
    if v <= 0 {
        Trit::F
    } else if v >= 2 {
        Trit::T
    } else {
        Trit::U
    }
}

pub open spec fn knot(t: Trit) -> Trit {
    match t {
        Trit::F => Trit::T,
        Trit::U => Trit::U,
        Trit::T => Trit::F,
    }
}

/// 格序 min（Ł3 与 K3 相同）。
pub open spec fn land(a: Trit, b: Trit) -> Trit {
    of_val(if val(a) < val(b) { val(a) } else { val(b) })
}

/// 格序 max。
pub open spec fn lor(a: Trit, b: Trit) -> Trit {
    of_val(if val(a) > val(b) { val(a) } else { val(b) })
}

/// 强加法 `min(1, a+b)`。
pub open spec fn sadd(a: Trit, b: Trit) -> Trit {
    of_val(if val(a) + val(b) > 2 { 2 } else { val(a) + val(b) })
}

/// 强合取 `max(0, a+b-1)`。
pub open spec fn sand(a: Trit, b: Trit) -> Trit {
    of_val(if val(a) + val(b) - 2 < 0 { 0 } else { val(a) + val(b) - 2 })
}

/// MV 蕴含 `min(1, 1-a+b)`。
pub open spec fn timp(a: Trit, b: Trit) -> Trit {
    of_val(if 2 - val(a) + val(b) > 2 { 2 } else { 2 - val(a) + val(b) })
}

/// 等价：`(a ⇒ b) ∧ (b ⇒ a)`。
pub open spec fn tequiv(a: Trit, b: Trit) -> Trit {
    land(timp(a, b), timp(b, a))
}

// ---- 证明 ---------------------------------------------------------------------

pub proof fn knot_involution(t: Trit)
    ensures knot(knot(t)) == t,
{
    match t {
        Trit::F => {},
        Trit::U => {},
        Trit::T => {},
    }
}

pub proof fn land_commutative(a: Trit, b: Trit)
    ensures land(a, b) == land(b, a),
{
    assert(land(a, b) == land(b, a));
}

pub proof fn lor_commutative(a: Trit, b: Trit)
    ensures lor(a, b) == lor(b, a),
{
    assert(lor(a, b) == lor(b, a));
}

pub proof fn de_morgan_land(a: Trit, b: Trit)
    ensures knot(land(a, b)) == lor(knot(a), knot(b)),
{
    assert(knot(land(a, b)) == lor(knot(a), knot(b)));
}

pub proof fn de_morgan_lor(a: Trit, b: Trit)
    ensures knot(lor(a, b)) == land(knot(a), knot(b)),
{
    assert(knot(lor(a, b)) == land(knot(a), knot(b)));
}

pub proof fn sadd_commutative(a: Trit, b: Trit)
    ensures sadd(a, b) == sadd(b, a),
{
    assert(sadd(a, b) == sadd(b, a));
}

/// Ł3 的关键差异：`U ⊕ U = T`。
pub proof fn sadd_unknown_unknown()
    ensures sadd(Trit::U, Trit::U) == Trit::T,
{
    assert(sadd(Trit::U, Trit::U) == Trit::T);
}

pub proof fn sand_commutative(a: Trit, b: Trit)
    ensures sand(a, b) == sand(b, a),
{
    assert(sand(a, b) == sand(b, a));
}

/// Ł3 的关键差异：`U ⊗ U = F`。
pub proof fn sand_unknown_unknown()
    ensures sand(Trit::U, Trit::U) == Trit::F,
{
    assert(sand(Trit::U, Trit::U) == Trit::F);
}

/// Ł3 的 `U ⇒ U = T`（K3/B3 是 U）。
pub proof fn timp_unknown_unknown()
    ensures timp(Trit::U, Trit::U) == Trit::T,
{
    assert(timp(Trit::U, Trit::U) == Trit::T);
}

pub proof fn timp_false_left(b: Trit)
    ensures timp(Trit::F, b) == Trit::T,
{
    assert(timp(Trit::F, b) == Trit::T);
}

pub proof fn timp_true_right(a: Trit)
    ensures timp(a, Trit::T) == Trit::T,
{
    assert(timp(a, Trit::T) == Trit::T);
}

pub proof fn equiv_reflexive(t: Trit)
    ensures tequiv(t, t) == Trit::T,
{
    assert(tequiv(t, t) == Trit::T);
}


// ---- exec 提炼 ---------------------------------------------------------------

pub fn knot_exec(t: Trit) -> (r: Trit)
    ensures r == knot(t),
{
    match t {
        Trit::F => Trit::T,
        Trit::U => Trit::U,
        Trit::T => Trit::F,
    }
}

pub fn land_exec(a: Trit, b: Trit) -> (r: Trit)
    ensures r == land(a, b),
{
    match (a, b) {
        (Trit::F, _) => Trit::F,
        (_, Trit::F) => Trit::F,
        (Trit::T, Trit::T) => Trit::T,
        _ => Trit::U,
    }
}

pub fn lor_exec(a: Trit, b: Trit) -> (r: Trit)
    ensures r == lor(a, b),
{
    match (a, b) {
        (Trit::T, _) => Trit::T,
        (_, Trit::T) => Trit::T,
        (Trit::F, Trit::F) => Trit::F,
        _ => Trit::U,
    }
}

pub fn sadd_exec(a: Trit, b: Trit) -> (r: Trit)
    ensures r == sadd(a, b),
{
    match (a, b) {
        (Trit::F, Trit::F) => Trit::F,
        (Trit::F, Trit::U) => Trit::U,
        (Trit::F, Trit::T) => Trit::T,
        (Trit::U, Trit::F) => Trit::U,
        (Trit::U, Trit::U) => Trit::T,
        (Trit::U, Trit::T) => Trit::T,
        (Trit::T, Trit::F) => Trit::T,
        (Trit::T, Trit::U) => Trit::T,
        (Trit::T, Trit::T) => Trit::T,
    }
}

pub fn sand_exec(a: Trit, b: Trit) -> (r: Trit)
    ensures r == sand(a, b),
{
    match (a, b) {
        (Trit::F, _) => Trit::F,
        (_, Trit::F) => Trit::F,
        (Trit::U, Trit::U) => Trit::F,
        (Trit::U, Trit::T) => Trit::U,
        (Trit::T, Trit::U) => Trit::U,
        (Trit::T, Trit::T) => Trit::T,
    }
}

pub fn timp_exec(a: Trit, b: Trit) -> (r: Trit)
    ensures r == timp(a, b),
{
    match (a, b) {
        (Trit::F, _) => Trit::T,
        (_, Trit::T) => Trit::T,
        (Trit::U, Trit::U) => Trit::T,
        (Trit::U, Trit::F) => Trit::U,
        (Trit::T, Trit::F) => Trit::F,
        (Trit::T, Trit::U) => Trit::U,
    }
}

pub fn tequiv_exec(a: Trit, b: Trit) -> (r: Trit)
    ensures r == tequiv(a, b),
{
    land_exec(timp_exec(a, b), timp_exec(b, a))
}

} // verus!
