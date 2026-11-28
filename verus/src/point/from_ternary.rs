//! `from_ternary` 的逐点打包（拆自 `point.rs`，过 per-file 行数台账）。
//!
//! 逐位调用已证的 `set_planes`，用 `prefix` 循环不变式把「前 `k` 位等于输入」
//! 推到整向量；未知平面按「全零 ⇒ `None`」归一。
use vstd::prelude::*;

#[cfg(verus_only)]
use crate::bulk::{tri_at, tri_of_word, unknown_word, unknowns_view, view, wf, words_for};
use crate::k3::Trit;
use super::set_planes;
#[cfg(verus_only)]
use super::{bit_at_zero, set_at_spec};

verus! {

/// 前 `k` 位取 `values`、其余为 `F` 的视图（`from_ternary` 的循环不变式用）。
pub open spec fn prefix(n: nat, values: Seq<Trit>, k: int) -> Seq<Trit> {
    Seq::new(n, |j: int| if j < k { values[j] } else { Trit::F })
}

/// 从标量三值序列打包（主库 `from_ternary`；未知平面按需归一）。
///
/// 主库用惰性 `get_or_insert_with` 逐位写平面；这里为复用已证的 `set_planes`
/// 单点等价，写成「逐点调用」的形式（验证工作区不追性能，语义一致）。
pub fn from_ternary_planes(values: &Vec<Trit>) -> (r: (Vec<u64>, Option<Vec<u64>>))
    ensures
        view(values@.len(), r.0@, unknowns_view(&r.1)) == values@,
        wf(values@.len(), r.0@, unknowns_view(&r.1)),
{
    let n = values.len();
    let words: usize = n / 64 + (if n % 64 == 0 { 0usize } else { 1usize });
    let mut ones: Vec<u64> = Vec::new();
    let mut unk: Option<Vec<u64>> = None;
    let mut i: usize = 0;
    while i < words
        invariant
            ones@.len() == i,
            forall|k: int| 0 <= k < i as int ==> #[trigger] ones@[k] == 0u64,
            0 <= i <= words,
            words == n / 64 + (if n % 64 == 0 { 0usize } else { 1usize }),
        decreases words - i,
    {
        ones.push(0u64);
        i = i + 1;
    }
    proof {
        assert(words as nat == words_for(n as nat));
        assert(ones@.len() == words);
        assert(unknowns_view(&unk) == None);
        assert forall|w: int| 0 <= w < words as nat implies #[trigger] ones@[w] == 0u64 by {
            assert(ones@[w] == 0u64);
        }
        assert forall|j: int| 0 <= j < n as nat implies tri_at(
            ones@,
            None,
            j,
        ) == Trit::F by {
            assert(ones@[j / 64] == 0u64);
            bit_at_zero((j % 64) as u32);
            assert(tri_of_word(0u64, 0u64, (j % 64) as u32) == Trit::F);
        }
        assert(view(n as nat, ones@, unknowns_view(&unk)) == prefix(
            n as nat,
            values@,
            0,
        ));
        assert forall|w: int| 0 <= w < words_for(n as nat) implies #[trigger] ones@[w] == 0u64 by {
            assert(w < words as int);
            assert(ones@[w] == 0u64);
        }
        crate::point::wf_lemmas::wf_none(n as nat, ones@);
        assert(unknowns_view(&unk) == None);
        assert(wf(n as nat, ones@, unknowns_view(&unk)));
    }
    let mut k: usize = 0;
    while k < n
        invariant
            n == values@.len(),
            ones@.len() == words,
            match unk {
                Some(p) => p@.len() == words,
                None => true,
            },
            words as nat == words_for(n as nat),
            wf(n as nat, ones@, unknowns_view(&unk)),
            forall|kk: int|
                0 <= kk < words_for(n as nat) ==> #[trigger] (ones@[kk] & unknown_word(
                    unknowns_view(&unk),
                    kk,
                )) == 0u64,
            view(n as nat, ones@, unknowns_view(&unk)) == prefix(n as nat, values@, k as int),
            0 <= k <= n,
        decreases n - k,
    {
        let value = match &values[k] {
            Trit::F => Trit::F,
            Trit::T => Trit::T,
            Trit::U => Trit::U,
        };
        let r = set_planes(n, &ones, &unk, k, value);
        proof {
            assert(value == values@[k as int]);
            assert(view(n as nat, r.0@, unknowns_view(&r.1)) == set_at_spec(
                n as nat,
                ones@,
                unknowns_view(&unk),
                k as int,
                values@[k as int],
            ));

            assert(view(n as nat, r.0@, unknowns_view(&r.1)) == prefix(
                n as nat,
                values@,
                k as int + 1,
            )) by {
                assert forall|j: int| 0 <= j < n as nat implies set_at_spec(
                    n as nat,
                    ones@,
                    unknowns_view(&unk),
                    k as int,
                    values@[k as int],
                )[j] == prefix(
                    n as nat,
                    values@,
                    k as int + 1,
                )[j] by {
                    if j < k {
                        assert(prefix(n as nat, values@, k as int)[j] == values@[j]);
                        assert(prefix(
                            n as nat,
                            values@,
                            k as int + 1,
                        )[j] == values@[j]);
                    } else if j == k {
                        assert(prefix(
                            n as nat,
                            values@,
                            k as int + 1,
                        )[j] == values@[j]);
                    } else {
                        assert(prefix(n as nat, values@, k as int)[j] == Trit::F);
                        assert(prefix(
                            n as nat,
                            values@,
                            k as int + 1,
                        )[j] == Trit::F);
                    }
                }
            }
        }
        ones = r.0;
        unk = r.1;
        k = k + 1;
    }
    proof {
        assert(view(n as nat, ones@, unknowns_view(&unk)) == prefix(n as nat, values@, n as int));
        assert(prefix(n as nat, values@, n as int) == values@) by {
            assert forall|j: int| 0 <= j < n as nat implies prefix(
                n as nat,
                values@,
                n as int,
            )[j] == values@[j] by {
            }
        }
        assert(wf(n as nat, ones@, unknowns_view(&unk)));
    }
    (ones, unk)
}

}
