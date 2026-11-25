//! Bochvar 三值（B3）的 Verus 规格与证明。
//!
//! B3 把 `U` 当作毒值：任何含 `U` 的运算结果都是 `U`；不含 `U` 时退化为经典二值。

use vstd::prelude::*;

verus! {

/// B3 三值。
#[derive(PartialEq, Eq, Clone, Copy)]
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

pub open spec fn band(a: Trit, b: Trit) -> Trit {
    if a == Trit::U || b == Trit::U {
        Trit::U
    } else if a == Trit::T && b == Trit::T {
        Trit::T
    } else {
        Trit::F
    }
}

pub open spec fn bor(a: Trit, b: Trit) -> Trit {
    if a == Trit::U || b == Trit::U {
        Trit::U
    } else if a == Trit::T || b == Trit::T {
        Trit::T
    } else {
        Trit::F
    }
}

/// B3 蕴含：`U` 吸收；否则 `T ⇒ b = b`、`F ⇒ b = T`。
pub open spec fn bimp(a: Trit, b: Trit) -> Trit {
    if a == Trit::U || b == Trit::U {
        Trit::U
    } else if a == Trit::T {
        b
    } else {
        Trit::T
    }
}

/// B3 的强加法与强合取：非 `U` 部分就是合取。
pub open spec fn sadd(a: Trit, b: Trit) -> Trit {
    band(a, b)
}

pub open spec fn sand(a: Trit, b: Trit) -> Trit {
    band(a, b)
}

pub open spec fn tequiv(a: Trit, b: Trit) -> Trit {
    band(bimp(a, b), bimp(b, a))
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

/// 毒值：任一操作数为 U，结果必为 U。
pub proof fn band_poison_left(b: Trit)
    ensures band(Trit::U, b) == Trit::U,
{
    assert(band(Trit::U, b) == Trit::U);
}

pub proof fn band_poison_right(a: Trit)
    ensures band(a, Trit::U) == Trit::U,
{
    assert(band(a, Trit::U) == Trit::U);
}

pub proof fn bor_poison_left(b: Trit)
    ensures bor(Trit::U, b) == Trit::U,
{
    assert(bor(Trit::U, b) == Trit::U);
}

pub proof fn bimp_poison_left(b: Trit)
    ensures bimp(Trit::U, b) == Trit::U,
{
    assert(bimp(Trit::U, b) == Trit::U);
}

pub proof fn bimp_poison_right(a: Trit)
    ensures bimp(a, Trit::U) == Trit::U,
{
    assert(bimp(a, Trit::U) == Trit::U);
}

pub proof fn band_commutative(a: Trit, b: Trit)
    ensures band(a, b) == band(b, a),
{
    assert(band(a, b) == band(b, a));
}

pub proof fn bor_commutative(a: Trit, b: Trit)
    ensures bor(a, b) == bor(b, a),
{
    assert(bor(a, b) == bor(b, a));
}

pub proof fn de_morgan_band(a: Trit, b: Trit)
    ensures knot(band(a, b)) == bor(knot(a), knot(b)),
{
    assert(knot(band(a, b)) == bor(knot(a), knot(b)));
}

pub proof fn de_morgan_bor(a: Trit, b: Trit)
    ensures knot(bor(a, b)) == band(knot(a), knot(b)),
{
    assert(knot(bor(a, b)) == band(knot(a), knot(b)));
}

/// 非 `U` 的 `T ∧ x = x`。
pub proof fn band_true_left(b: Trit)
    ensures b != Trit::U ==> band(Trit::T, b) == b,
{
    assert(b != Trit::U ==> band(Trit::T, b) == b);
}

/// B3 的 `U ⇒ U = U`（Ł3 是 T）。
pub proof fn bimp_unknown_unknown()
    ensures bimp(Trit::U, Trit::U) == Trit::U,
{
    assert(bimp(Trit::U, Trit::U) == Trit::U);
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

pub fn band_exec(a: Trit, b: Trit) -> (r: Trit)
    ensures r == band(a, b),
{
    match (a, b) {
        (Trit::U, _) => Trit::U,
        (_, Trit::U) => Trit::U,
        (Trit::T, Trit::T) => Trit::T,
        _ => Trit::F,
    }
}

pub fn bor_exec(a: Trit, b: Trit) -> (r: Trit)
    ensures r == bor(a, b),
{
    match (a, b) {
        (Trit::U, _) => Trit::U,
        (_, Trit::U) => Trit::U,
        (Trit::T, _) => Trit::T,
        (_, Trit::T) => Trit::T,
        _ => Trit::F,
    }
}

pub fn bimp_exec(a: Trit, b: Trit) -> (r: Trit)
    ensures r == bimp(a, b),
{
    match (a, b) {
        (Trit::U, _) => Trit::U,
        (_, Trit::U) => Trit::U,
        (Trit::T, b) => b,
        (Trit::F, _) => Trit::T,
    }
}

pub fn sadd_exec(a: Trit, b: Trit) -> (r: Trit)
    ensures r == sadd(a, b),
{
    band_exec(a, b)
}

pub fn sand_exec(a: Trit, b: Trit) -> (r: Trit)
    ensures r == sand(a, b),
{
    band_exec(a, b)
}

pub fn tequiv_exec(a: Trit, b: Trit) -> (r: Trit)
    ensures r == tequiv(a, b),
{
    band_exec(bimp_exec(a, b), bimp_exec(b, a))
}

} // verus!
