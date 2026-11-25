//! `TriVec` 的整向量等价证明（`bulk.rs` 的子模块）。
//!
//! 目标：批量平面运算的结果视图 `view(len, out_plan, unknowns)` 与逐元素标量
//! 运算一致。`bulk.rs` 的 exec 已证「结果平面 == `*_words` spec」，这里证
//! 「`*_words` spec 的视图 == 逐元素标量运算」，复用 `bits/proofs.rs` 里已证的
//! `TriBit::not_get`。

use vstd::prelude::*;

#[cfg(verus_only)]
use crate::bits::bit_at;
#[cfg(verus_only)]
use crate::bits::{BitPlane, TriBit};
#[cfg(verus_only)]
use crate::bulk::{
    all_zero_plane, and_ones_b3, and_ones_lattice, and_unknowns_b3, and_unknowns_lattice,
    mask_word, not_words, or_ones_b3, or_ones_lattice, or_unknowns_b3,
    or_unknowns_lattice, poison_and, poison_or, set_at_spec, set_ones_word,
    set_unknowns_word, tri_at, tri_of_word, unknown_word, view, wf, words_for,
};
#[cfg(verus_only)]
use crate::k3::{knot, tand, tor, Trit};

verus! {

/// `!(o | x)` 与 `TriBit::not_spec` 的 ones 平面形式等价。
proof fn not_ones_form(o: u64, x: u64)
    ensures
        !(o | x) == !(o & !x) & !x,
{
    assert(!(o | x) == !(o & !x) & !x) by (bit_vector);
}

/// `not` 的整向量等价：结果视图逐位等于标量 `knot`。
pub proof fn not_view_equiv(len: nat, ones: Seq<u64>, unknowns: Option<Seq<u64>>)
    requires
        wf(len, ones, unknowns),
    ensures
        view(len, not_words(len, ones, unknowns), unknowns) == Seq::new(
            len,
            |i: int| knot(tri_at(ones, unknowns, i)),
        ),
{
    let out = not_words(len, ones, unknowns);
    assert forall|i: int| 0 <= i < len implies #[trigger] tri_at(out, unknowns, i) == knot(
        tri_at(ones, unknowns, i),
    ) by {
        let w = i / 64;
        let bit = (i % 64) as u32;
        assert(words_for(len) == (len + 63) / 64);
        assert(0 <= w < words_for(len));
        assert(w * 64 + (bit as int) == i);
        let o = ones[w];
        let x = unknown_word(unknowns, w);
        let before = TriBit { ones: BitPlane(o), unknowns: BitPlane(x) };
        let after = TriBit { ones: BitPlane(out[w]), unknowns: BitPlane(x) };
        assert(out[w] == !(o | x));
        not_ones_form(o, x);
        // 逐字结果平面与 `not_spec` 的 ones 平面相同，unknowns 平面保持不变。
        assert(after == before.not_spec());
        before.not_get(bit);
        assert(after.get(bit) == knot(before.get(bit)));
        // 两端与 `tri_at` 对齐。
        assert(before.get(bit) == tri_at(ones, unknowns, i));
        assert(after.get(bit) == tri_at(out, unknowns, i));
    }
}

/// `wf` 下 `crisp_ones` 退化为 `ones` 本身，`crisp_zeros` 保持。
proof fn lattice_planes_eq(oa: u64, xa: u64, ob: u64, xb: u64)
    requires
        oa & xa == 0,
        ob & xb == 0,
    ensures
        ((oa & !xa) & (ob & !xb)) == (oa & ob),
        ((oa & !xa) | (ob & !xb)) == (oa | ob),
{
    assert(
        ((oa & xa == 0u64) && (ob & xb == 0u64)) ==> (((oa & !xa) & (ob & !xb)) == (oa & ob))
    ) by (bit_vector);
    assert(
        ((oa & xa == 0u64) && (ob & xb == 0u64)) ==> (((oa & !xa) | (ob & !xb)) == (oa | ob))
    ) by (bit_vector);
}

/// De Morgan：`!(a | b) == !a & !b`。
proof fn de_morgan_concat(a: u64, b: u64)
    ensures
        !(a | b) == !a & !b,
{
    assert(!(a | b) == !a & !b) by (bit_vector);
}

/// `and`（K3/Ł3）的整向量等价：结果视图逐位等于标量 `tand`。
pub proof fn and_view_equiv(
    len: nat,
    oa: Seq<u64>,
    xa: Option<Seq<u64>>,
    ob: Seq<u64>,
    xb: Option<Seq<u64>>,
)
    requires
        wf(len, oa, xa),
        wf(len, ob, xb),
    ensures
        view(
            len,
            Seq::new(
                words_for(len),
                |w: int| and_ones_lattice(oa, xa, ob, xb, w),
            ),
            Some(
                Seq::new(
                    words_for(len),
                    |w: int| and_unknowns_lattice(oa, xa, ob, xb, w),
                ),
            ),
        ) == Seq::new(len, |i: int| tand(tri_at(oa, xa, i), tri_at(ob, xb, i))),
{
    let os = Seq::new(words_for(len), |w: int| and_ones_lattice(oa, xa, ob, xb, w));
    let xs = Seq::new(words_for(len), |w: int| and_unknowns_lattice(oa, xa, ob, xb, w));
    assert forall|i: int| 0 <= i < len implies #[trigger] tri_at(os, Some(xs), i) == tand(
        tri_at(oa, xa, i),
        tri_at(ob, xb, i),
    ) by {
        let w = i / 64;
        let bit = (i % 64) as u32;
        assert(words_for(len) == (len + 63) / 64);
        assert(0 <= w < words_for(len));
        assert(w * 64 + (bit as int) == i);
        let o1 = oa[w];
        let o2 = ob[w];
        let x1 = unknown_word(xa, w);
        let x2 = unknown_word(xb, w);
        let a = TriBit { ones: BitPlane(o1), unknowns: BitPlane(x1) };
        let b = TriBit { ones: BitPlane(o2), unknowns: BitPlane(x2) };
        let r = TriBit { ones: BitPlane(os[w]), unknowns: BitPlane(xs[w]) };
        // `wf` 给出两个平面在 `len` 内互斥。
        assert((o1 & x1) == 0 && (o2 & x2) == 0);
        let f = (!o1 & !x1) | (!o2 & !x2);
        let o_out = o1 & o2;
        assert(a.crisp_ones() == o1 & !x1);
        assert(b.crisp_ones() == o2 & !x2);
        assert(a.crisp_zeros() == !o1 & !x1);
        assert(b.crisp_zeros() == !o2 & !x2);
        lattice_planes_eq(o1, x1, o2, x2);
        assert(os[w] == o_out);
        assert(xs[w] == !(o_out | f));
        assert((a.crisp_ones() & b.crisp_ones()) == o_out);
        assert((a.crisp_zeros() | b.crisp_zeros()) == f);
        assert(!(o_out | f) == !o_out & !f) by (bit_vector);
        assert((!o_out & !f) == (!f & !o_out)) by (bit_vector);
        assert(a.and_lattice_spec(b).ones.0 == o_out);
        assert(a.and_lattice_spec(b).unknowns.0 == (!f & !o_out));
        assert(r == a.and_lattice_spec(b));
        a.and_lattice_get(b, bit);
        assert(r.get(bit) == tand(a.get(bit), b.get(bit)));
        assert(a.get(bit) == tri_at(oa, xa, i));
        assert(b.get(bit) == tri_at(ob, xb, i));
        assert(r.get(bit) == tri_at(os, Some(xs), i));
    }
}

/// `or`（K3/Ł3）的整向量等价：结果视图逐位等于标量 `tor`。
pub proof fn or_view_equiv(
    len: nat,
    oa: Seq<u64>,
    xa: Option<Seq<u64>>,
    ob: Seq<u64>,
    xb: Option<Seq<u64>>,
)
    requires
        wf(len, oa, xa),
        wf(len, ob, xb),
    ensures
        view(
            len,
            Seq::new(
                words_for(len),
                |w: int| or_ones_lattice(oa, xa, ob, xb, w),
            ),
            Some(
                Seq::new(
                    words_for(len),
                    |w: int| or_unknowns_lattice(oa, xa, ob, xb, w),
                ),
            ),
        ) == Seq::new(len, |i: int| tor(tri_at(oa, xa, i), tri_at(ob, xb, i))),
{
    let os = Seq::new(words_for(len), |w: int| or_ones_lattice(oa, xa, ob, xb, w));
    let xs = Seq::new(words_for(len), |w: int| or_unknowns_lattice(oa, xa, ob, xb, w));
    assert forall|i: int| 0 <= i < len implies #[trigger] tri_at(os, Some(xs), i) == tor(
        tri_at(oa, xa, i),
        tri_at(ob, xb, i),
    ) by {
        let w = i / 64;
        let bit = (i % 64) as u32;
        assert(words_for(len) == (len + 63) / 64);
        assert(0 <= w < words_for(len));
        assert(w * 64 + (bit as int) == i);
        let o1 = oa[w];
        let o2 = ob[w];
        let x1 = unknown_word(xa, w);
        let x2 = unknown_word(xb, w);
        let a = TriBit { ones: BitPlane(o1), unknowns: BitPlane(x1) };
        let b = TriBit { ones: BitPlane(o2), unknowns: BitPlane(x2) };
        let r = TriBit { ones: BitPlane(os[w]), unknowns: BitPlane(xs[w]) };
        assert((o1 & x1) == 0 && (o2 & x2) == 0);
        let f = (!o1 & !x1) & (!o2 & !x2);
        let o_out = o1 | o2;
        assert(a.crisp_ones() == o1 & !x1);
        assert(b.crisp_ones() == o2 & !x2);
        assert(a.crisp_zeros() == !o1 & !x1);
        assert(b.crisp_zeros() == !o2 & !x2);
        lattice_planes_eq(o1, x1, o2, x2);
        assert(os[w] == o_out);
        assert(xs[w] == !(o_out | f));
        assert((a.crisp_ones() | b.crisp_ones()) == o_out);
        assert((a.crisp_zeros() & b.crisp_zeros()) == f);
        assert(!(o_out | f) == !o_out & !f) by (bit_vector);
        assert(a.or_lattice_spec(b).ones.0 == o_out);
        assert(a.or_lattice_spec(b).unknowns.0 == xs[w]);
        assert(r == a.or_lattice_spec(b));
        a.or_lattice_get(b, bit);
        assert(r.get(bit) == tor(a.get(bit), b.get(bit)));
        assert(a.get(bit) == tri_at(oa, xa, i));
        assert(b.get(bit) == tri_at(ob, xb, i));
        assert(r.get(bit) == tri_at(os, Some(xs), i));
    }
}

/// 或的逐位分解（`bit_vector` 支持的形式）。
proof fn or_bit(a: u64, b: u64, bit: u32)
    ensures
        bit_at(a | b, bit) == (bit_at(a, bit) || bit_at(b, bit)),
{
    assert(bit_at(a | b, bit) == (bit_at(a, bit) || bit_at(b, bit))) by (bit_vector);
}

/// 与的逐位分解。
proof fn and_bit(a: u64, b: u64, bit: u32)
    ensures
        bit_at(a & b, bit) == (bit_at(a, bit) && bit_at(b, bit)),
{
    assert(bit_at(a & b, bit) == (bit_at(a, bit) && bit_at(b, bit))) by (bit_vector);
}

/// 取反的逐位分解。
proof fn not_bit(a: u64, bit: u32)
    ensures
        bit_at(!a, bit) == !bit_at(a, bit),
{
    assert(bit_at(!a, bit) == !bit_at(a, bit)) by (bit_vector);
}

/// `tri_at` 与字级 `tri_of_word` 对齐。
proof fn tri_at_word(ones: Seq<u64>, unknowns: Option<Seq<u64>>, i: int)
    requires
        0 <= i,
    ensures
        tri_at(ones, unknowns, i) == tri_of_word(
            ones[i / 64],
            unknown_word(unknowns, i / 64),
            (i % 64) as u32,
        ),
{
}

/// `and`（B3 毒值）的整向量等价。
pub proof fn and_view_equiv_b3(
    len: nat,
    oa: Seq<u64>,
    xa: Option<Seq<u64>>,
    ob: Seq<u64>,
    xb: Option<Seq<u64>>,
)
    requires
        wf(len, oa, xa),
        wf(len, ob, xb),
    ensures
        view(
            len,
            Seq::new(words_for(len), |w: int| and_ones_b3(oa, xa, ob, xb, w)),
            Some(Seq::new(words_for(len), |w: int| and_unknowns_b3(oa, xa, ob, xb, w))),
        ) == Seq::new(len, |i: int| poison_and(tri_at(oa, xa, i), tri_at(ob, xb, i))),
{
    let os = Seq::new(words_for(len), |w: int| and_ones_b3(oa, xa, ob, xb, w));
    let xs = Seq::new(words_for(len), |w: int| and_unknowns_b3(oa, xa, ob, xb, w));
    assert forall|i: int| 0 <= i < len implies #[trigger] tri_at(os, Some(xs), i) == poison_and(
        tri_at(oa, xa, i),
        tri_at(ob, xb, i),
    ) by {
        let w = i / 64;
        let bit = (i % 64) as u32;
        assert(words_for(len) == (len + 63) / 64);
        assert(0 <= w < words_for(len));
        assert(w * 64 + (bit as int) == i);
        let o1 = oa[w];
        let o2 = ob[w];
        let x1 = unknown_word(xa, w);
        let x2 = unknown_word(xb, w);
        let xo = x1 | x2;
        assert(xs[w] == xo);
        assert(os[w] == (o1 & o2) & !xo);
        let a = tri_of_word(o1, x1, bit);
        let b = tri_of_word(o2, x2, bit);
        let r = tri_of_word(os[w], xo, bit);
        tri_at_word(oa, xa, i);
        tri_at_word(ob, xb, i);
        tri_at_word(os, Some(xs), i);
        assert(tri_at(oa, xa, i) == a);
        assert(tri_at(ob, xb, i) == b);
        assert(tri_at(os, Some(xs), i) == r);
        or_bit(x1, x2, bit);
        and_bit(o1, o2, bit);
        not_bit(xo, bit);
        if bit_at(xo, bit) {
            assert(a == Trit::U || b == Trit::U);
            assert(r == Trit::U);
        } else {
            assert(a != Trit::U && b != Trit::U);
            and_bit(o1 & o2, !xo, bit);
            assert(bit_at(!xo, bit) == !bit_at(xo, bit));
            assert(bit_at(os[w], bit) == (bit_at(o1, bit) && bit_at(o2, bit)));
            if bit_at(o1, bit) && bit_at(o2, bit) {
                assert(a == Trit::T && b == Trit::T);
                assert(r == Trit::T);
            } else {
                assert(r == Trit::F);
            }
        }
        assert(r == poison_and(a, b));
    }
}

/// `or`（B3 毒值）的整向量等价。
pub proof fn or_view_equiv_b3(
    len: nat,
    oa: Seq<u64>,
    xa: Option<Seq<u64>>,
    ob: Seq<u64>,
    xb: Option<Seq<u64>>,
)
    requires
        wf(len, oa, xa),
        wf(len, ob, xb),
    ensures
        view(
            len,
            Seq::new(words_for(len), |w: int| or_ones_b3(oa, xa, ob, xb, w)),
            Some(Seq::new(words_for(len), |w: int| or_unknowns_b3(oa, xa, ob, xb, w))),
        ) == Seq::new(len, |i: int| poison_or(tri_at(oa, xa, i), tri_at(ob, xb, i))),
{    let os = Seq::new(words_for(len), |w: int| or_ones_b3(oa, xa, ob, xb, w));
    let xs = Seq::new(words_for(len), |w: int| or_unknowns_b3(oa, xa, ob, xb, w));
    assert forall|i: int| 0 <= i < len implies #[trigger] tri_at(os, Some(xs), i) == poison_or(
        tri_at(oa, xa, i),
        tri_at(ob, xb, i),
    ) by {
        let w = i / 64;
        let bit = (i % 64) as u32;
        assert(words_for(len) == (len + 63) / 64);
        assert(0 <= w < words_for(len));
        assert(w * 64 + (bit as int) == i);
        let o1 = oa[w];
        let o2 = ob[w];
        let x1 = unknown_word(xa, w);
        let x2 = unknown_word(xb, w);
        let xo = x1 | x2;
        assert(xs[w] == xo);
        assert(os[w] == (o1 | o2) & !xo);
        let a = tri_of_word(o1, x1, bit);
        let b = tri_of_word(o2, x2, bit);
        let r = tri_of_word(os[w], xo, bit);
        tri_at_word(oa, xa, i);
        tri_at_word(ob, xb, i);
        tri_at_word(os, Some(xs), i);
        assert(tri_at(oa, xa, i) == a);
        assert(tri_at(ob, xb, i) == b);
        assert(tri_at(os, Some(xs), i) == r);
        or_bit(x1, x2, bit);
        or_bit(o1, o2, bit);
        not_bit(xo, bit);
        if bit_at(xo, bit) {
            assert(a == Trit::U || b == Trit::U);
            assert(r == Trit::U);
        } else {
            assert(a != Trit::U && b != Trit::U);
            and_bit(o1 | o2, !xo, bit);
            assert(bit_at(!xo, bit) == !bit_at(xo, bit));
            assert(bit_at(os[w], bit) == (bit_at(o1, bit) || bit_at(o2, bit)));
            if bit_at(o1, bit) || bit_at(o2, bit) {
                assert(a == Trit::T || b == Trit::T);
                assert(r == Trit::T);
            } else {
                assert(r == Trit::F);
            }
        }
        assert(r == poison_or(a, b));
    }
}

// ---- 单点操作 `set` / `from_ternary` 的视图 lemma -------------------------

/// 掩码字的第 `b` 位：仅当命中目标字、且 `bit` 的该位为 1 时为真。
pub proof fn mask_bit_at(w: int, word: int, bit: u64, b: u32)
    requires
        b < 64,
    ensures
        bit_at(mask_word(w, word, bit), b) == (w == word && bit_at(bit, b)),
{
    if w == word {
        assert(mask_word(w, word, bit) == bit);
    } else {
        assert(mask_word(w, word, bit) == 0u64);
        assert(bit_at(0u64, b) == false) by (bit_vector);
    }
}

/// `bit == 1 << s` 时，`bit` 的第 `b` 位为 1 当且仅当 `b == s`（两者都小于 64）。
pub proof fn bit_self(s: u32, bit: u64, b: u32)
    requires
        s < 64,
        b < 64,
        bit == 1u64 << s,
    ensures
        bit_at(bit, b) == (b == s),
{
    assert(bit_at(1u64 << s, b) == (b == s)) by (bit_vector)
        requires
            s < 64,
            b < 64,
    ;
}

/// 两位相同则字级三值相同。
pub proof fn tri_of_word_ext(o1: u64, x1: u64, o2: u64, x2: u64, b: u32)
    requires
        bit_at(o1, b) == bit_at(o2, b),
        bit_at(x1, b) == bit_at(x2, b),
    ensures
        tri_of_word(o1, x1, b) == tri_of_word(o2, x2, b),
{
}

/// 写命中目标位：两个结果平面在该位的三值就是写入值。
pub proof fn set_target_forms(o: u64, x: u64, m: u64, b: u32, v: Trit)
    requires
        bit_at(m, b),
    ensures
        tri_of_word(set_ones_word(o, m, v), set_unknowns_word(x, m, v), b) == v,
{
    match v {
        Trit::F => {
            assert(bit_at(m, b) ==> !bit_at(o & !m, b)) by (bit_vector);
            assert(bit_at(m, b) ==> !bit_at(x & !m, b)) by (bit_vector);
            assert(set_ones_word(o, m, v) == o & !m);
            assert(set_unknowns_word(x, m, v) == x & !m);
            assert(tri_of_word(o & !m, x & !m, b) == Trit::F);
        }
        Trit::T => {
            assert(bit_at(m, b) ==> bit_at(o | m, b)) by (bit_vector);
            assert(bit_at(m, b) ==> !bit_at(x & !m, b)) by (bit_vector);
            assert(set_ones_word(o, m, v) == o | m);
            assert(set_unknowns_word(x, m, v) == x & !m);
            assert(tri_of_word(o | m, x & !m, b) == Trit::T);
        }
        Trit::U => {
            assert(bit_at(m, b) ==> !bit_at(o & !m, b)) by (bit_vector);
            assert(bit_at(m, b) ==> bit_at(x | m, b)) by (bit_vector);
            assert(set_ones_word(o, m, v) == o & !m);
            assert(set_unknowns_word(x, m, v) == x | m);
            assert(tri_of_word(o & !m, x | m, b) == Trit::U);
        }
    }
}

/// 写不命中目标位：两个结果平面在该位的位值不变。
pub proof fn set_other_forms(o: u64, x: u64, m: u64, b: u32, v: Trit)
    requires
        !bit_at(m, b),
    ensures
        bit_at(set_ones_word(o, m, v), b) == bit_at(o, b),
        bit_at(set_unknowns_word(x, m, v), b) == bit_at(x, b),
{
    assert(!bit_at(m, b) ==> (bit_at(o & !m, b) == bit_at(o, b))) by (bit_vector);
    assert(!bit_at(m, b) ==> (bit_at(o | m, b) == bit_at(o, b))) by (bit_vector);
    assert(!bit_at(m, b) ==> (bit_at(x & !m, b) == bit_at(x, b))) by (bit_vector);
    assert(!bit_at(m, b) ==> (bit_at(x | m, b) == bit_at(x, b))) by (bit_vector);
    match v {
        Trit::F => {
            assert(set_ones_word(o, m, v) == o & !m);
            assert(set_unknowns_word(x, m, v) == x & !m);
        }
        Trit::T => {
            assert(set_ones_word(o, m, v) == o | m);
            assert(set_unknowns_word(x, m, v) == x & !m);
        }
        Trit::U => {
            assert(set_ones_word(o, m, v) == o & !m);
            assert(set_unknowns_word(x, m, v) == x | m);
        }
    }
}

/// 单点写的整向量等价：结果视图 = 第 `i` 位替换成 `v` 的视图。
pub proof fn set_view_equiv(
    len: nat,
    oa: Seq<u64>,
    xa: Option<Seq<u64>>,
    i: int,
    word: int,
    sh: u32,
    bit: u64,
    v: Trit,
    oo: Seq<u64>,
    xo: Option<Seq<u64>>,
)
    requires
        0 <= i < len,
        word == i / 64,
        sh == (i % 64) as u32,
        bit == 1u64 << sh,
        oo.len() == words_for(len),
        match xo {
            Some(p) => p.len() == words_for(len),
            None => true,
        },
        forall|w: int|
            0 <= w < words_for(len) ==> #[trigger] oo[w] == set_ones_word(
                oa[w],
                mask_word(w, word, bit),
                v,
            ),
        forall|w: int|
            0 <= w < words_for(len) ==> #[trigger] unknown_word(xo, w)
                == set_unknowns_word(unknown_word(xa, w), mask_word(w, word, bit), v),
    ensures
        view(len, oo, xo) == set_at_spec(len, oa, xa, i, v),
{
    assert forall|k: int| 0 <= k < len implies #[trigger] tri_at(oo, xo, k) == (
        if k == i {
            v
        } else {
            tri_at(oa, xa, k)
        }
    ) by {
        let w = k / 64;
        let b = (k % 64) as u32;
        assert(0 <= w < words_for(len));
        assert(w * 64 + (b as int) == k);
        tri_at_word(oa, xa, k);
        assert(tri_at(oa, xa, k) == tri_of_word(oa[w], unknown_word(xa, w), b));
        assert(tri_at(oo, xo, k) == tri_of_word(oo[w], unknown_word(xo, w), b));
        assert(oo[w] == set_ones_word(oa[w], mask_word(w, word, bit), v));
        assert(unknown_word(xo, w) == set_unknowns_word(
            unknown_word(xa, w),
            mask_word(w, word, bit),
            v,
        ));
        mask_bit_at(w, word, bit, b);
        if k == i {
            assert(b == sh);
            bit_self(sh, bit, b);
            assert(bit_at(bit, b));
            assert(bit_at(mask_word(w, word, bit), b));
            set_target_forms(oa[w], unknown_word(xa, w), mask_word(w, word, bit), b, v);
            assert(tri_of_word(oo[w], unknown_word(xo, w), b) == v);
        } else {
            if w == word {
                assert(b != sh);
                bit_self(sh, bit, b);
                assert(!bit_at(bit, b));
            }
            assert(!bit_at(mask_word(w, word, bit), b));
            set_other_forms(oa[w], unknown_word(xa, w), mask_word(w, word, bit), b, v);
            tri_of_word_ext(
                oo[w],
                unknown_word(xo, w),
                oa[w],
                unknown_word(xa, w),
                b,
            );
            assert(tri_of_word(oo[w], unknown_word(xo, w), b) == tri_of_word(
                oa[w],
                unknown_word(xa, w),
                b,
            ));
        }
    }
}

/// 单点写的结构不变式：目标字互斥，其它字由输入 `wf` 继承。
pub proof fn set_word_excl(o: u64, x: u64, m: u64, v: Trit)
    requires
        o & x == 0u64,
    ensures
        set_ones_word(o, m, v) & set_unknowns_word(x, m, v) == 0u64,
{
    match v {
        Trit::F => {
            assert((o & x == 0u64) ==> ((o & !m) & (x & !m) == 0u64)) by (bit_vector);
        }
        Trit::T => {
            assert((o & x == 0u64) ==> ((o | m) & (x & !m) == 0u64)) by (bit_vector);
        }
        Trit::U => {
            assert((o & x == 0u64) ==> ((o & !m) & (x | m) == 0u64)) by (bit_vector);
        }
    }
}

/// 全零 unknown 平面与 `None` 的视图相同。
pub proof fn view_zero_plane(len: nat, o: Seq<u64>, ps: Seq<u64>)
    requires
        ps.len() == words_for(len),
        all_zero_plane(ps),
    ensures
        view(len, o, Some(ps)) == view(len, o, None),
{
    assert forall|k: int| 0 <= k < len implies #[trigger] tri_at(o, Some(ps), k) == tri_at(
        o,
        None,
        k,
    ) by {
        assert(unknown_word(Some(ps), k / 64) == 0u64);
        assert(unknown_word(None, k / 64) == 0u64);
    }
}

}
