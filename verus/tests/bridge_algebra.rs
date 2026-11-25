//! `trit-verified` 的 Ł3 / B3 / Belnap 四值 exec 提炼与主库实现逐项对拍。
//!
//! 三值是有限域（3×3 = 9 组），四值是 4×4 = 16 组；全部穷举。

use trit_belnap::B4;
use trit_core::algebra::Algebra;
use trit_core::value::Ternary;
use trit_verified::{b3, b4, l3};

fn enc_l3(t: l3::Trit) -> Ternary {
    match t {
        l3::Trit::F => Ternary::False,
        l3::Trit::U => Ternary::Unknown,
        l3::Trit::T => Ternary::True,
    }
}

fn dec_l3(t: Ternary) -> l3::Trit {
    match t {
        Ternary::False => l3::Trit::F,
        Ternary::Unknown => l3::Trit::U,
        Ternary::True => l3::Trit::T,
    }
}

fn enc_b3(t: b3::Trit) -> Ternary {
    match t {
        b3::Trit::F => Ternary::False,
        b3::Trit::U => Ternary::Unknown,
        b3::Trit::T => Ternary::True,
    }
}

fn dec_b3(t: Ternary) -> b3::Trit {
    match t {
        Ternary::False => b3::Trit::F,
        Ternary::Unknown => b3::Trit::U,
        Ternary::True => b3::Trit::T,
    }
}

fn enc_b4(v: b4::B4) -> B4 {
    match v {
        b4::B4::N => B4::N,
        b4::B4::F => B4::F,
        b4::B4::T => B4::T,
        b4::B4::B => B4::B,
    }
}

fn dec_b4(v: B4) -> b4::B4 {
    match v {
        B4::N => b4::B4::N,
        B4::F => b4::B4::F,
        B4::T => b4::B4::T,
        B4::B => b4::B4::B,
    }
}

#[test]
fn lukasiewicz_matches_trit_core() {
    let algebra = Algebra::Lukasiewicz;
    for a in Ternary::ALL {
        assert_eq!(enc_l3(l3::knot_exec(dec_l3(a))), algebra.not(a));
        for b in Ternary::ALL {
            let (x, y) = (dec_l3(a), dec_l3(b));
            assert_eq!(
                enc_l3(l3::land_exec(x, y)),
                algebra.and(a, b),
                "{a:?} & {b:?}"
            );
            assert_eq!(
                enc_l3(l3::lor_exec(x, y)),
                algebra.or(a, b),
                "{a:?} | {b:?}"
            );
            assert_eq!(
                enc_l3(l3::sadd_exec(x, y)),
                algebra.strong_add(a, b),
                "{a:?} + {b:?}"
            );
            assert_eq!(
                enc_l3(l3::sand_exec(x, y)),
                algebra.strong_and(a, b),
                "{a:?} * {b:?}"
            );
            assert_eq!(
                enc_l3(l3::timp_exec(x, y)),
                algebra.implies(a, b),
                "{a:?} => {b:?}"
            );
            assert_eq!(
                enc_l3(l3::tequiv_exec(x, y)),
                algebra.equiv(a, b),
                "{a:?} <=> {b:?}"
            );
        }
    }
}

#[test]
fn bochvar_matches_trit_core() {
    let algebra = Algebra::Bochvar;
    for a in Ternary::ALL {
        assert_eq!(enc_b3(b3::knot_exec(dec_b3(a))), algebra.not(a));
        for b in Ternary::ALL {
            let (x, y) = (dec_b3(a), dec_b3(b));
            assert_eq!(
                enc_b3(b3::band_exec(x, y)),
                algebra.and(a, b),
                "{a:?} & {b:?}"
            );
            assert_eq!(
                enc_b3(b3::bor_exec(x, y)),
                algebra.or(a, b),
                "{a:?} | {b:?}"
            );
            assert_eq!(
                enc_b3(b3::sadd_exec(x, y)),
                algebra.strong_add(a, b),
                "{a:?} + {b:?}"
            );
            assert_eq!(
                enc_b3(b3::sand_exec(x, y)),
                algebra.strong_and(a, b),
                "{a:?} * {b:?}"
            );
            assert_eq!(
                enc_b3(b3::bimp_exec(x, y)),
                algebra.implies(a, b),
                "{a:?} => {b:?}"
            );
            assert_eq!(
                enc_b3(b3::tequiv_exec(x, y)),
                algebra.equiv(a, b),
                "{a:?} <=> {b:?}"
            );
        }
    }
}

#[test]
fn belnap_matches_trit_belnap() {
    let all = [B4::N, B4::F, B4::T, B4::B];
    for a in all {
        assert_eq!(enc_b4(b4::knot_exec(dec_b4(a))), a.not(), "{a:?} !");
        assert_eq!(enc_b4(b4::conflation_exec(dec_b4(a))), a.conflation());
        for b in all {
            let (x, y) = (dec_b4(a), dec_b4(b));
            assert_eq!(enc_b4(b4::band_exec(x, y)), a.and(b), "{a:?} & {b:?}");
            assert_eq!(enc_b4(b4::bor_exec(x, y)), a.or(b), "{a:?} | {b:?}");
            assert_eq!(enc_b4(b4::bimp_exec(x, y)), a.implies(b), "{a:?} => {b:?}");
            assert_eq!(enc_b4(b4::tequiv_exec(x, y)), a.equiv(b), "{a:?} <=> {b:?}");
            assert_eq!(enc_b4(b4::txor_exec(x, y)), a.xor(b), "{a:?} ^ {b:?}");
            assert_eq!(
                enc_b4(b4::consensus_exec(x, y)),
                a.consensus(b),
                "{a:?} consensus {b:?}"
            );
            assert_eq!(
                enc_b4(b4::gullibility_exec(x, y)),
                a.gullibility(b),
                "{a:?} gullibility {b:?}"
            );
        }
    }
}
