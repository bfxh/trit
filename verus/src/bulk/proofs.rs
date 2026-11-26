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
    and_ones_b3, and_ones_lattice, and_unknowns_b3, and_unknowns_lattice, not_words, or_ones_b3,
    or_ones_lattice, or_unknowns_b3, or_unknowns_lattice, poison_and, poison_or, tri_at,
    tri_of_word, unknown_word, view, wf, words_for,
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
{
    let os = Seq::new(words_for(len), |w: int| or_ones_b3(oa, xa, ob, xb, w));
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

}
