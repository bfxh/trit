//! `wf`（两平面互斥）保持性 lemma（拆自 `point/proofs.rs`）。
//!
//! `set_word_wf`：单个字上写入任意三值后，两平面仍互斥；
//! `wf_none`：全零 `ones` 平面 + `None` 的未知平面满足 `wf`。

use vstd::prelude::*;

#[cfg(verus_only)]
use crate::bulk::{unknown_word, wf, words_for};
use crate::k3::Trit;
#[cfg(verus_only)]
use super::{set_ones_word, set_unknowns_word};

verus! {

/// 全零 `ones` 平面 + `None` 的 `unknowns` 满足结构不变式 `wf`。
pub proof fn wf_none(len: nat, ones: Seq<u64>)
    requires
        ones.len() == words_for(len),
        forall|w: int| 0 <= w < words_for(len) ==> #[trigger] ones[w] == 0u64,
    ensures
        wf(len, ones, None),
{
    assert forall|w: int| 0 <= w < words_for(len) implies #[trigger] (ones[w] & unknown_word(
        None,
        w,
    )) == 0u64 by {
        assert(ones[w] == 0u64);
        assert(unknown_word(None, w) == 0u64);
        assert((ones[w] & 0u64) == 0u64) by (bit_vector);
    }
}

/// 单字写保持「两平面互斥」：输入字互斥 ⇒ 写出字互斥（对任意掩码 `m`）。
pub proof fn set_word_wf(o: u64, x: u64, m: u64, v: Trit)
    requires
        (o & x) == 0u64,
    ensures
        (set_ones_word(o, m, v) & set_unknowns_word(x, m, v)) == 0u64,
{
    match v {
        Trit::F => {
            assert((set_ones_word(o, m, Trit::F) & set_unknowns_word(x, m, Trit::F)) == 0u64)
                by (bit_vector)
                requires
                    (o & x) == 0u64,
            ;
        }
        Trit::T => {
            assert((set_ones_word(o, m, Trit::T) & set_unknowns_word(x, m, Trit::T)) == 0u64)
                by (bit_vector)
                requires
                    (o & x) == 0u64,
            ;
        }
        Trit::U => {
            assert((set_ones_word(o, m, Trit::U) & set_unknowns_word(x, m, Trit::U)) == 0u64)
                by (bit_vector)
                requires
                    (o & x) == 0u64,
            ;
        }
    }
}

}
