//! `TriVec`（位切片三值向量）的单点操作：`get` / `set`（`from_ternary` 在
//! `point/from_ternary.rs`）。
//!
//! 语义口径：证「结果视图」与标量语义一致（`set_at_spec` / `prefix`），并保持
//! 结构不变式 `wf`（两平面互斥）；不逐字复刻主库的分支与惰性分配。位级推理统一
//! 落在 `u64` 上：掩码由 `mask_word(w, word, bit)` 显式参数化，避免 `int` 位移
//! 进入 bit-vector 求解；相关 lemma 在 `point/proofs.rs` 与 `point/wf_lemmas.rs`。
//!
//! 诚实边界：padding 位不在 `view` 语义内，由 `tests/bridge_point.rs` 对拍主库覆盖。

use vstd::prelude::*;

#[cfg(verus_only)]
use crate::bits::bit_at;
#[cfg(verus_only)]
use crate::bulk::{tri_at, tri_of_word, unknown_word, unknowns_view, view, wf, words_for};
use crate::k3::Trit;

verus! {

mod from_ternary;
mod proofs;
mod wf_lemmas;

/// 位提取 `(w >> b) & 1` 与 `bit_at` 对齐（`b < 64`）。
pub proof fn bit_at_extract(w: u64, b: u32)
    requires
        b < 64,
    ensures
        bit_at(w, b) == (((w >> b) & 1u64) == 1u64),
{
    assert(b % 64 == b);
    assert(bit_at(w, b) == (((w >> b) & 1u64) == 1u64));
}

/// 零字的任何一位都是 0（`b < 64`）。
pub proof fn bit_at_zero(b: u32)
    requires
        b < 64,
    ensures
        bit_at(0u64, b) == false,
{
    assert(bit_at(0u64, b) == false) by (bit_vector)
        requires
            b < 64,
    ;
}

// ---- 单点操作：`get` / `set` / `from_ternary` ------------------------------
//
// 语义口径：这里证的是“视图 + 结构不变式”，不逐字复刻主库的分支与惰性分配。
// 主库 `try_set` 在清掉最后一个 `Unknown` 时把平面置 `None`；本文件把它归一
// 成“全零平面 ⇒ `None`”，两者在 `view` / `wf` 下等价（`tests/bridge_bulk.rs`
// 对拍主库，覆盖归一化分支）。

/// 单点读的视图（`tri_at` 的别名，让 `ensures` 可读）。
pub open spec fn get_at_spec(
    len: nat,
    ones: Seq<u64>,
    unknowns: Option<Seq<u64>>,
    i: int,
) -> Trit {
    tri_at(ones, unknowns, i)
}

/// 单点写之后的视图：第 `i` 位换成 `v`，其余位保持。
pub open spec fn set_at_spec(
    len: nat,
    ones: Seq<u64>,
    unknowns: Option<Seq<u64>>,
    i: int,
    v: Trit,
) -> Seq<Trit> {
    Seq::new(len, |k: int| if k == i { v } else { tri_at(ones, unknowns, k) })
}

/// 第 `w` 个字的掩码：命中的字给位掩码 `bit`，其它字为 0。
///
/// 这里把「位掩码」作为参数传入（exec 侧算作 `1 << (i % 64)`，是 `u64` 值），
/// 好处是证明侧只需比较 `w == word` 与 `bit_at(bit, b)`，避免 `int` 位移进入
/// bit-vector 推理。
pub open spec fn mask_word(w: int, word: int, bit: u64) -> u64 {
    if w == word {
        bit
    } else {
        0u64
    }
}

/// 单点写后的 `ones` 字：`F`/`U` 清位，`T` 置位。
pub open spec fn set_ones_word(o: u64, m: u64, v: Trit) -> u64 {
    match v {
        Trit::F => o & !m,
        Trit::T => o | m,
        Trit::U => o & !m,
    }
}

/// 单点写后的 `unknowns` 字：`U` 置位，`F`/`T` 清位。
pub open spec fn set_unknowns_word(x: u64, m: u64, v: Trit) -> u64 {
    match v {
        Trit::F => x & !m,
        Trit::T => x & !m,
        Trit::U => x | m,
    }
}

/// 平面是否全零（用于把 `Some(全零)` 归一成 `None`）。
pub open spec fn all_zero_plane(ps: Seq<u64>) -> bool {
    forall|w: int| 0 <= w < ps.len() ==> #[trigger] ps[w] == 0u64
}

/// 单点读：主库 `try_get` 的位提取与三值判定。
pub fn get_at(
    len: usize,
    ones: &Vec<u64>,
    unknowns: &Option<Vec<u64>>,
    i: usize,
) -> (r: Trit)
    requires
        wf(len as nat, ones@, unknowns_view(unknowns)),
        i < len,
    ensures
        r == get_at_spec(len as nat, ones@, unknowns_view(unknowns), i as int),
{
    let shift: u32 = (i % 64) as u32;
    let o: u64 = (ones[i / 64] >> shift) & 1u64;
    let x: u64 = match unknowns {
        Some(p) => (p[i / 64] >> shift) & 1u64,
        None => 0u64,
    };
    proof {
        assert(0 <= (i / 64) as int);
        assert(((i / 64) as int) < words_for(len as nat));
        assert(((i / 64) as int) * 64 + (shift as int) == i as int);
        bit_at_extract(ones@[(i / 64) as int], shift);
        assert(bit_at(ones@[(i / 64) as int], shift) == (o == 1u64));
        match unknowns {
            Some(p) => {
                bit_at_extract(p@[(i / 64) as int], shift);
                assert(unknown_word(unknowns_view(unknowns), (i / 64) as int) == p@[(i / 64) as int]);
                assert(bit_at(unknown_word(unknowns_view(unknowns), (i / 64) as int), shift) == (x == 1u64));
            }
            None => {
                assert(unknown_word(unknowns_view(unknowns), (i / 64) as int) == 0u64);
                assert((((0u64 >> shift) & 1u64) == 1u64) == false) by (bit_vector);
                assert(bit_at(0u64, shift) == false);
            }
        }
    }
    if x == 1u64 {
        Trit::U
    } else if o == 1u64 {
        Trit::T
    } else {
        Trit::F
    }
}

/// 单点写：主库 `try_set` 的位运算（按字掩码，逐字推送两个结果平面）。
pub fn set_planes(
    len: usize,
    ones: &Vec<u64>,
    unknowns: &Option<Vec<u64>>,
    i: usize,
    value: Trit,
) -> (r: (Vec<u64>, Option<Vec<u64>>))
    requires
        wf(len as nat, ones@, unknowns_view(unknowns)),
        forall|k: int|
            0 <= k < words_for(len as nat) ==> #[trigger] (ones@[k] & unknown_word(
                unknowns_view(unknowns),
                k,
            )) == 0u64,
        i < len,
    ensures
        wf(len as nat, r.0@, unknowns_view(&r.1)),
        view(len as nat, r.0@, unknowns_view(&r.1)) == set_at_spec(
            len as nat,
            ones@,
            unknowns_view(unknowns),
            i as int,
            value,
        ),
{
    let n = ones.len();
    let word: usize = i / 64;
    let shift: u32 = (i % 64) as u32;
    let bit: u64 = 1u64 << shift;
    let mut out_ones: Vec<u64> = Vec::new();
    let mut out_unknowns: Vec<u64> = Vec::new();
    for w in 0..n
        invariant
            i < len,
            forall|k: int|
                0 <= k < words_for(len as nat) ==> #[trigger] (ones@[k] & unknown_word(
                    unknowns_view(unknowns),
                    k,
                )) == 0u64,
            ones@.len() == n,
            n as nat == words_for(len as nat),
            word as int == i as int / 64,
            shift == (i % 64) as u32,
            bit == 1u64 << shift,
            match unknowns {
                Some(p) => p@.len() == n,
                None => true,
            },
            out_ones@.len() == w,
            out_unknowns@.len() == w,
            0 <= w <= n,
            forall|k: int|
                0 <= k < w ==> #[trigger] (out_ones@[k] & out_unknowns@[k]) == 0u64,
            forall|k: int|
                0 <= k < w ==> #[trigger] out_ones@[k] == set_ones_word(
                    ones@[k],
                    mask_word(k, word as int, bit),
                    value,
                ),
            forall|k: int|
                0 <= k < w ==> #[trigger] out_unknowns@[k] == set_unknowns_word(
                    unknown_word(unknowns_view(unknowns), k),
                    mask_word(k, word as int, bit),
                    value,
                ),
    {
        let o = ones[w];
        let x: u64 = match unknowns {
            Some(p) => p[w],
            None => 0u64,
        };
        let m: u64 = if w == word { bit } else { 0u64 };
        proof {
            assert(mask_word(w as int, word as int, bit) == m);
        }
        let o2 = match value {
            Trit::F => o & !m,
            Trit::T => o | m,
            Trit::U => o & !m,
        };
        let x2 = match value {
            Trit::F => x & !m,
            Trit::T => x & !m,
            Trit::U => x | m,
        };
        out_ones.push(o2);
        out_unknowns.push(x2);
        proof {
            let wi: int = w as int;
            assert(0 <= wi);
            assert(wi < words_for(len as nat));
            assert((ones@[wi] & unknown_word(unknowns_view(unknowns), wi)) == 0u64);
            assert(unknown_word(unknowns_view(unknowns), wi) == x);
            assert((o & x) == 0u64);
            wf_lemmas::set_word_wf(o, x, m, value);
            assert((o2 & x2) == 0u64);
            assert forall|k: int|
                0 <= k < w + 1 implies #[trigger] (out_ones@[k] & out_unknowns@[k]) == 0u64 by {
                if k < w {
                } else {
                    assert(k == w);
                    assert(out_ones@[k] == o2);
                    assert(out_unknowns@[k] == x2);
                }
            }
        }
    }
    let mut any: bool = false;
    let mut idx: usize = 0;
    while idx < n
        invariant
            out_unknowns@.len() == n,
            0 <= idx <= n,
            any == (exists|k: int| 0 <= k < idx && #[trigger] out_unknowns@[k] != 0u64),
        decreases n - idx,
    {
        if out_unknowns[idx] != 0u64 {
            any = true;
        }
        idx = idx + 1;
    }
    let r1: Option<Vec<u64>> = if any { Some(out_unknowns) } else { None };
    proof {
        assert(out_ones@.len() == words_for(len as nat));
        if any {
            assert(out_unknowns@.len() == words_for(len as nat));
        }
        assert forall|w: int|
            0 <= w < words_for(len as nat) implies #[trigger] out_ones@[w] == set_ones_word(
                ones@[w],
                mask_word(w, word as int, bit),
                value,
            ) by {
        }
        assert forall|w: int|
            0 <= w < words_for(len as nat) implies #[trigger] unknown_word(
                unknowns_view(&r1),
                w,
            ) == set_unknowns_word(
                unknown_word(unknowns_view(unknowns), w),
                mask_word(w, word as int, bit),
                value,
            ) by {
            if any {
                assert(unknown_word(unknowns_view(&r1), w) == out_unknowns@[w]);
            } else {
                assert(unknown_word(unknowns_view(&r1), w) == 0u64);
                assert(!(exists|k: int|
                    0 <= k < n && #[trigger] out_unknowns@[k] != 0u64));
                assert(out_unknowns@[w] == 0u64);
            }
        }
        proofs::set_view_equiv(
            len as nat,
            ones@,
            unknowns_view(unknowns),
            i as int,
            word as int,
            shift,
            bit,
            value,
            out_ones@,
            unknowns_view(&r1),
        );
        assert forall|k: int|
            0 <= k < words_for(len as nat) implies #[trigger] (out_ones@[k] & unknown_word(
                unknowns_view(&r1),
                k,
            )) == 0u64 by {
            if any {
                assert(unknown_word(unknowns_view(&r1), k) == out_unknowns@[k]);
            } else {
                assert(unknown_word(unknowns_view(&r1), k) == 0u64);
                assert(out_unknowns@[k] == 0u64);
            }
        }
    }
    (out_ones, r1)
}
}
