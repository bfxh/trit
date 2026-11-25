//! 单点操作的位级 lemma：掩码命中、写入命中/未命中位的三值、整向量等价。
//!
//! 约定：`bit == 1 << s`（`s < 64`）时第 `s` 位为 1；掩码 `mask_word(w, word, bit)`
//! 在 `w == word` 处取 `bit`，否则为 0。

use vstd::prelude::*;

#[cfg(verus_only)]
use super::{mask_word, set_at_spec, set_ones_word, set_unknowns_word};
#[cfg(verus_only)]
use crate::bits::bit_at;
#[cfg(verus_only)]
use crate::bulk::{tri_at, tri_of_word, unknown_word, view, words_for};
use crate::k3::Trit;

verus! {

/// `tri_at` 展开成字级 `tri_of_word`（`i` 的非法值不在此列）。
pub proof fn tri_at_word(ones: Seq<u64>, unknowns: Option<Seq<u64>>, i: int)
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

}
