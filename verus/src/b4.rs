//! Belnap 四值（B4：N/F/T/B）的 Verus 规格与证明。
//!
//! 双轨表示：`T = (1,0)`、`F = (0,1)`、`B = (1,1)`、`N = (0,0)`。
//! 两轨都是 `bool`，所以代数律可以直接落到布尔代数上，SMT 自动证明。

use vstd::prelude::*;

verus! {

/// Belnap 四值。
#[derive(PartialEq, Eq, Clone, Copy)]
pub enum B4 {
    /// 两者都不是（neither）。
    N,
    /// 假。
    F,
    /// 真。
    T,
    /// 同时为真和为假（both）。
    B,
}

pub open spec fn t_rail(v: B4) -> bool {
    v == B4::T || v == B4::B
}

pub open spec fn f_rail(v: B4) -> bool {
    v == B4::F || v == B4::B
}

pub open spec fn of_rails(t: bool, f: bool) -> B4 {
    match (t, f) {
        (true, false) => B4::T,
        (false, true) => B4::F,
        (true, true) => B4::B,
        (false, false) => B4::N,
    }
}

/// 否定：交换两轨。
pub open spec fn knot(v: B4) -> B4 {
    of_rails(f_rail(v), t_rail(v))
}

/// 合取：`t = t₁∧t₂`，`f = f₁∨f₂`。
pub open spec fn band(a: B4, b: B4) -> B4 {
    of_rails(t_rail(a) && t_rail(b), f_rail(a) || f_rail(b))
}

/// 析取：`t = t₁∨t₂`，`f = f₁∧f₂`。
pub open spec fn bor(a: B4, b: B4) -> B4 {
    of_rails(t_rail(a) || t_rail(b), f_rail(a) && f_rail(b))
}

/// 蕴含：`¬a ∨ b`。
pub open spec fn bimp(a: B4, b: B4) -> B4 {
    bor(knot(a), b)
}

/// 等价：`(a ⇒ b) ∧ (b ⇒ a)`。
pub open spec fn tequiv(a: B4, b: B4) -> B4 {
    band(bimp(a, b), bimp(b, a))
}

/// 异或：`¬(a ⇔ b)`。
pub open spec fn txor(a: B4, b: B4) -> B4 {
    knot(tequiv(a, b))
}

/// 知识序合取（共识）：两轨各自 AND。
pub open spec fn consensus(a: B4, b: B4) -> B4 {
    of_rails(t_rail(a) && t_rail(b), f_rail(a) && f_rail(b))
}

/// 知识序析取（轻信）：两轨各自 OR。
pub open spec fn gullibility(a: B4, b: B4) -> B4 {
    of_rails(t_rail(a) || t_rail(b), f_rail(a) || f_rail(b))
}

/// 共轭：`¬ₖ`，交换知识序两端。
pub open spec fn conflation(v: B4) -> B4 {
    of_rails(!f_rail(v), !t_rail(v))
}

// ---- 证明 ---------------------------------------------------------------------

/// 双轨往返：任何 B4 都能由自己的两轨还原。
pub proof fn rails_roundtrip(v: B4)
    ensures of_rails(t_rail(v), f_rail(v)) == v,
{
    match v {
        B4::N => {},
        B4::F => {},
        B4::T => {},
        B4::B => {},
    }
}

pub proof fn knot_involution(v: B4)
    ensures knot(knot(v)) == v,
{
    assert(knot(knot(v)) == v);
}

pub proof fn band_commutative(a: B4, b: B4)
    ensures band(a, b) == band(b, a),
{
    assert(band(a, b) == band(b, a));
}

pub proof fn bor_commutative(a: B4, b: B4)
    ensures bor(a, b) == bor(b, a),
{
    assert(bor(a, b) == bor(b, a));
}

pub proof fn de_morgan_band(a: B4, b: B4)
    ensures knot(band(a, b)) == bor(knot(a), knot(b)),
{
    assert(knot(band(a, b)) == bor(knot(a), knot(b)));
}

pub proof fn de_morgan_bor(a: B4, b: B4)
    ensures knot(bor(a, b)) == band(knot(a), knot(b)),
{
    assert(knot(bor(a, b)) == band(knot(a), knot(b)));
}

pub proof fn band_associative(a: B4, b: B4, c: B4)
    ensures band(band(a, b), c) == band(a, band(b, c)),
{
    assert(band(band(a, b), c) == band(a, band(b, c)));
}

pub proof fn bor_associative(a: B4, b: B4, c: B4)
    ensures bor(bor(a, b), c) == bor(a, bor(b, c)),
{
    assert(bor(bor(a, b), c) == bor(a, bor(b, c)));
}

pub proof fn band_distributes_over_bor(a: B4, b: B4, c: B4)
    ensures band(a, bor(b, c)) == bor(band(a, b), band(a, c)),
{
    assert(band(a, bor(b, c)) == bor(band(a, b), band(a, c)));
}

pub proof fn bor_distributes_over_band(a: B4, b: B4, c: B4)
    ensures bor(a, band(b, c)) == band(bor(a, b), bor(a, c)),
{
    assert(bor(a, band(b, c)) == band(bor(a, b), bor(a, c)));
}

pub proof fn band_absorption(a: B4, b: B4)
    ensures band(a, bor(a, b)) == a,
{
    assert(band(a, bor(a, b)) == a);
}

pub proof fn bor_absorption(a: B4, b: B4)
    ensures bor(a, band(a, b)) == a,
{
    assert(bor(a, band(a, b)) == a);
}

pub proof fn txor_is_not_equiv(a: B4, b: B4)
    ensures txor(a, b) == knot(tequiv(a, b)),
{
}

pub proof fn consensus_commutative(a: B4, b: B4)
    ensures consensus(a, b) == consensus(b, a),
{
    assert(consensus(a, b) == consensus(b, a));
}

pub proof fn gullibility_commutative(a: B4, b: B4)
    ensures gullibility(a, b) == gullibility(b, a),
{
    assert(gullibility(a, b) == gullibility(b, a));
}

pub proof fn conflation_involution(v: B4)
    ensures conflation(conflation(v)) == v,
{
    assert(conflation(conflation(v)) == v);
}

/// 知识序 De Morgan：共轭把 consensus 与 gullibility 对偶。
pub proof fn conflation_consensus(a: B4, b: B4)
    ensures conflation(consensus(a, b)) == gullibility(conflation(a), conflation(b)),
{
    assert(conflation(consensus(a, b)) == gullibility(conflation(a), conflation(b)));
}

/// 真序与知识序的端点是 N/B，`T`/`F` 不动。
pub proof fn conflation_endpoints()
    ensures
        conflation(B4::N) == B4::B,
        conflation(B4::B) == B4::N,
        conflation(B4::T) == B4::T,
        conflation(B4::F) == B4::F,
{
    assert(conflation(B4::N) == B4::B);
    assert(conflation(B4::B) == B4::N);
    assert(conflation(B4::T) == B4::T);
    assert(conflation(B4::F) == B4::F);
}

/// `N` 是知识序的底，`B` 是知识序的顶。
pub proof fn knowledge_endpoints(a: B4)
    ensures
        consensus(a, B4::B) == a,
        gullibility(a, B4::N) == a,
{
    assert(consensus(a, B4::B) == a);
    assert(gullibility(a, B4::N) == a);
}


// ---- exec 提炼 ---------------------------------------------------------------

fn t_rail_exec(v: B4) -> (r: bool)
    ensures r == t_rail(v),
{
    match v {
        B4::T | B4::B => true,
        B4::N | B4::F => false,
    }
}

fn f_rail_exec(v: B4) -> (r: bool)
    ensures r == f_rail(v),
{
    match v {
        B4::F | B4::B => true,
        B4::N | B4::T => false,
    }
}

fn of_rails_exec(t: bool, f: bool) -> (r: B4)
    ensures r == of_rails(t, f),
{
    match (t, f) {
        (true, false) => B4::T,
        (false, true) => B4::F,
        (true, true) => B4::B,
        (false, false) => B4::N,
    }
}

pub fn knot_exec(v: B4) -> (r: B4)
    ensures r == knot(v),
{
    of_rails_exec(f_rail_exec(v), t_rail_exec(v))
}

pub fn band_exec(a: B4, b: B4) -> (r: B4)
    ensures r == band(a, b),
{
    of_rails_exec(
        t_rail_exec(a) && t_rail_exec(b),
        f_rail_exec(a) || f_rail_exec(b),
    )
}

pub fn bor_exec(a: B4, b: B4) -> (r: B4)
    ensures r == bor(a, b),
{
    of_rails_exec(
        t_rail_exec(a) || t_rail_exec(b),
        f_rail_exec(a) && f_rail_exec(b),
    )
}

pub fn bimp_exec(a: B4, b: B4) -> (r: B4)
    ensures r == bimp(a, b),
{
    bor_exec(knot_exec(a), b)
}

pub fn tequiv_exec(a: B4, b: B4) -> (r: B4)
    ensures r == tequiv(a, b),
{
    band_exec(bimp_exec(a, b), bimp_exec(b, a))
}

pub fn txor_exec(a: B4, b: B4) -> (r: B4)
    ensures r == txor(a, b),
{
    knot_exec(tequiv_exec(a, b))
}

pub fn consensus_exec(a: B4, b: B4) -> (r: B4)
    ensures r == consensus(a, b),
{
    of_rails_exec(
        t_rail_exec(a) && t_rail_exec(b),
        f_rail_exec(a) && f_rail_exec(b),
    )
}

pub fn gullibility_exec(a: B4, b: B4) -> (r: B4)
    ensures r == gullibility(a, b),
{
    of_rails_exec(
        t_rail_exec(a) || t_rail_exec(b),
        f_rail_exec(a) || f_rail_exec(b),
    )
}

pub fn conflation_exec(v: B4) -> (r: B4)
    ensures r == conflation(v),
{
    of_rails_exec(!f_rail_exec(v), !t_rail_exec(v))
}

} // verus!
