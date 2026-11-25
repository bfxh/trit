//! 位切片三值向量 `TriVec` 的 spec/exec；整向量等价证明在 `bulk/proofs.rs`。
//!
//! 表示与主库 `crates/trit-core/src/bulk.rs` 一致：`len` 位，`ones` 平面
//! （1 = `True`），可选 `unknowns` 平面（1 = `Unknown`）。结构不变式 `wf`：
//! 两个平面在 `len` 位内互斥（每元素至多一个平面为 1），这是主库
//! `from_ternary` / `try_set` 维持的性质。
//!
//! 语义提炼边界：主库 `not` / `and_assign` / `or_assign` 会在最后一个字上
//! 用掩码清零 `len` 之外的 padding 位；这里不做该掩码，因为 [`view`] 只读
//! `len` 位内的位，padding 不在语义里。padding 的清理由
//! `tests/bridge_bulk.rs` 对拍主库实现覆盖。

use vstd::prelude::*;

#[cfg(verus_only)]
use crate::bits::bit_at;
#[cfg(verus_only)]
use crate::bits::{BitPlane, TriBit};
#[cfg(verus_only)]
use crate::k3::Trit;

verus! {

mod proofs;

/// `len` 位需要的 64 位字数。
pub open spec fn words_for(len: nat) -> nat {
    (len + 63) / 64
}

/// `unknowns` 平面的 spec 视图（`None` 表示没有该平面）。
pub open spec fn unknowns_view(unknowns: &Option<Vec<u64>>) -> Option<Seq<u64>> {
    match unknowns {
        Some(p) => Some(p@),
        None => None,
    }
}

/// 取 `unknowns` 平面的第 `w` 个字；`None` 视为全零平面。
pub open spec fn unknown_word(unknowns: Option<Seq<u64>>, w: int) -> u64 {
    match unknowns {
        Some(p) => p[w],
        None => 0u64,
    }
}

/// 第 `i` 位的三值。
pub open spec fn tri_at(ones: Seq<u64>, unknowns: Option<Seq<u64>>, i: int) -> Trit {
    if bit_at(unknown_word(unknowns, i / 64), (i % 64) as u32) {
        Trit::U
    } else if bit_at(ones[i / 64], (i % 64) as u32) {
        Trit::T
    } else {
        Trit::F
    }
}

/// 单个字内第 `bit` 位的三值（`tri_at` 的字级形式）。
pub open spec fn tri_of_word(o: u64, x: u64, bit: u32) -> Trit {
    if bit_at(x, bit) {
        Trit::U
    } else if bit_at(o, bit) {
        Trit::T
    } else {
        Trit::F
    }
}

/// 整向量视图：逐元素取三值。
pub open spec fn view(len: nat, ones: Seq<u64>, unknowns: Option<Seq<u64>>) -> Seq<Trit> {
    Seq::new(len, |i: int| tri_at(ones, unknowns, i))
}

/// 结构不变式：字数够用，且两个平面在 `len` 位内互斥。
pub open spec fn wf(len: nat, ones: Seq<u64>, unknowns: Option<Seq<u64>>) -> bool {
    &&& ones.len() == words_for(len)
    &&& match unknowns {
        Some(p) => p.len() == words_for(len),
        None => true,
    }
    &&& forall|w: int|
        0 <= w < words_for(len) ==> #[trigger] (ones[w] & unknown_word(unknowns, w)) == 0u64
}

/// `not` 的 `ones` 结果平面：`True`/`False` 互换、`Unknown` 不变。
pub open spec fn not_words(len: nat, ones: Seq<u64>, unknowns: Option<Seq<u64>>) -> Seq<u64> {
    Seq::new(words_for(len), |w: int| !(ones[w] | unknown_word(unknowns, w)))
}

/// 逐字取反后的 `ones` 平面（`unknowns` 平面不变）。
#[allow(unused_variables)]
pub fn not_planes(len: usize, ones: &Vec<u64>, unknowns: &Option<Vec<u64>>) -> (r: Vec<u64>)
    requires
        wf(len as nat, ones@, unknowns_view(unknowns)),
    ensures
        r@ == not_words(len as nat, ones@, unknowns_view(unknowns)),
{
    let n = ones.len();
    let mut out: Vec<u64> = Vec::new();
    for w in 0..n
        invariant
            ones@.len() == n,
            n as nat == words_for(len as nat),
            match unknowns {
                Some(p) => p@.len() == n,
                None => true,
            },
            out@.len() == w,
            forall|k: int| 0 <= k < w ==> #[trigger] out@[k] == not_words(
                len as nat,
                ones@,
                unknowns_view(unknowns),
            )[k],
    {
        let x: u64 = match unknowns {
            Some(p) => p[w],
            None => 0u64,
        };
        out.push(!(ones[w] | x));
    }
    out
}

// ---- 逐字二元运算：K3 / Ł3 格序与 B3 毒值 -----------------------------------

/// K3/Ł3 格序 `and` 的 `ones` 结果平面（逐字）。
pub open spec fn and_ones_lattice(
    oa: Seq<u64>,
    xa: Option<Seq<u64>>,
    ob: Seq<u64>,
    xb: Option<Seq<u64>>,
    w: int,
) -> u64 {
    oa[w] & ob[w]
}

/// K3/Ł3 格序 `and` 的 `unknowns` 结果平面（逐字）。
pub open spec fn and_unknowns_lattice(
    oa: Seq<u64>,
    xa: Option<Seq<u64>>,
    ob: Seq<u64>,
    xb: Option<Seq<u64>>,
    w: int,
) -> u64 {
    let o_out = oa[w] & ob[w];
    let f_out = (!oa[w] & !unknown_word(xa, w)) | (!ob[w] & !unknown_word(xb, w));
    !(o_out | f_out)
}

/// K3/Ł3 格序 `or` 的 `ones` 结果平面（逐字）。
pub open spec fn or_ones_lattice(
    oa: Seq<u64>,
    xa: Option<Seq<u64>>,
    ob: Seq<u64>,
    xb: Option<Seq<u64>>,
    w: int,
) -> u64 {
    oa[w] | ob[w]
}

/// K3/Ł3 格序 `or` 的 `unknowns` 结果平面（逐字）。
pub open spec fn or_unknowns_lattice(
    oa: Seq<u64>,
    xa: Option<Seq<u64>>,
    ob: Seq<u64>,
    xb: Option<Seq<u64>>,
    w: int,
) -> u64 {
    let o_out = oa[w] | ob[w];
    let f_out = (!oa[w] & !unknown_word(xa, w)) & (!ob[w] & !unknown_word(xb, w));
    !(o_out | f_out)
}

/// K3/Ł3 格序 `and`：逐字计算两个结果平面。
#[allow(unused_variables)]
pub fn and_planes_lattice(
    len: usize,
    ones: &Vec<u64>,
    unknowns: &Option<Vec<u64>>,
    other_ones: &Vec<u64>,
    other_unknowns: &Option<Vec<u64>>,
) -> (r: (Vec<u64>, Vec<u64>))
    requires
        wf(len as nat, ones@, unknowns_view(unknowns)),
        wf(len as nat, other_ones@, unknowns_view(other_unknowns)),
    ensures
        r.0@ == Seq::new(
            words_for(len as nat),
            |w: int| and_ones_lattice(
                ones@,
                unknowns_view(unknowns),
                other_ones@,
                unknowns_view(other_unknowns),
                w,
            ),
        ),
        r.1@ == Seq::new(
            words_for(len as nat),
            |w: int| and_unknowns_lattice(
                ones@,
                unknowns_view(unknowns),
                other_ones@,
                unknowns_view(other_unknowns),
                w,
            ),
        ),
{
    let n = ones.len();
    let mut out_ones: Vec<u64> = Vec::new();
    let mut out_unknowns: Vec<u64> = Vec::new();
    for w in 0..n
        invariant
            ones@.len() == n,
            other_ones@.len() == n,
            n as nat == words_for(len as nat),
            match unknowns {
                Some(p) => p@.len() == n,
                None => true,
            },
            match other_unknowns {
                Some(p) => p@.len() == n,
                None => true,
            },
            out_ones@.len() == w,
            out_unknowns@.len() == w,
            forall|k: int| 0 <= k < w ==> #[trigger] out_ones@[k] == and_ones_lattice(
                ones@,
                unknowns_view(unknowns),
                other_ones@,
                unknowns_view(other_unknowns),
                k,
            ),
            forall|k: int| 0 <= k < w ==> #[trigger] out_unknowns@[k] == and_unknowns_lattice(
                ones@,
                unknowns_view(unknowns),
                other_ones@,
                unknowns_view(other_unknowns),
                k,
            ),
    {
        let o1 = ones[w];
        let o2 = other_ones[w];
        let x1: u64 = match unknowns {
            Some(p) => p[w],
            None => 0u64,
        };
        let x2: u64 = match other_unknowns {
            Some(p) => p[w],
            None => 0u64,
        };
        let o_out = o1 & o2;
        let f_out = (!o1 & !x1) | (!o2 & !x2);
        out_ones.push(o_out);
        out_unknowns.push(!(o_out | f_out));
    }
    (out_ones, out_unknowns)
}

/// K3/Ł3 格序 `or`：逐字计算两个结果平面。
#[allow(unused_variables)]
pub fn or_planes_lattice(
    len: usize,
    ones: &Vec<u64>,
    unknowns: &Option<Vec<u64>>,
    other_ones: &Vec<u64>,
    other_unknowns: &Option<Vec<u64>>,
) -> (r: (Vec<u64>, Vec<u64>))
    requires
        wf(len as nat, ones@, unknowns_view(unknowns)),
        wf(len as nat, other_ones@, unknowns_view(other_unknowns)),
    ensures
        r.0@ == Seq::new(
            words_for(len as nat),
            |w: int| or_ones_lattice(
                ones@,
                unknowns_view(unknowns),
                other_ones@,
                unknowns_view(other_unknowns),
                w,
            ),
        ),
        r.1@ == Seq::new(
            words_for(len as nat),
            |w: int| or_unknowns_lattice(
                ones@,
                unknowns_view(unknowns),
                other_ones@,
                unknowns_view(other_unknowns),
                w,
            ),
        ),
{
    let n = ones.len();
    let mut out_ones: Vec<u64> = Vec::new();
    let mut out_unknowns: Vec<u64> = Vec::new();
    for w in 0..n
        invariant
            ones@.len() == n,
            other_ones@.len() == n,
            n as nat == words_for(len as nat),
            match unknowns {
                Some(p) => p@.len() == n,
                None => true,
            },
            match other_unknowns {
                Some(p) => p@.len() == n,
                None => true,
            },
            out_ones@.len() == w,
            out_unknowns@.len() == w,
            forall|k: int| 0 <= k < w ==> #[trigger] out_ones@[k] == or_ones_lattice(
                ones@,
                unknowns_view(unknowns),
                other_ones@,
                unknowns_view(other_unknowns),
                k,
            ),
            forall|k: int| 0 <= k < w ==> #[trigger] out_unknowns@[k] == or_unknowns_lattice(
                ones@,
                unknowns_view(unknowns),
                other_ones@,
                unknowns_view(other_unknowns),
                k,
            ),
    {
        let o1 = ones[w];
        let o2 = other_ones[w];
        let x1: u64 = match unknowns {
            Some(p) => p[w],
            None => 0u64,
        };
        let x2: u64 = match other_unknowns {
            Some(p) => p[w],
            None => 0u64,
        };
        let o_out = o1 | o2;
        let f_out = (!o1 & !x1) & (!o2 & !x2);
        out_ones.push(o_out);
        out_unknowns.push(!(o_out | f_out));
    }
    (out_ones, out_unknowns)
}

// ---- B3 毒值（Bochvar）：沾上 Unknown 即为 Unknown -------------------------

/// B3 毒值 `and`：任一 `U` → `U`；否则 `T ∧ T`。
pub open spec fn poison_and(a: Trit, b: Trit) -> Trit {
    if a == Trit::U || b == Trit::U {
        Trit::U
    } else if a == Trit::T && b == Trit::T {
        Trit::T
    } else {
        Trit::F
    }
}

/// B3 毒值 `or`：任一 `U` → `U`；否则 `T ∨ T`。
pub open spec fn poison_or(a: Trit, b: Trit) -> Trit {
    if a == Trit::U || b == Trit::U {
        Trit::U
    } else if a == Trit::T || b == Trit::T {
        Trit::T
    } else {
        Trit::F
    }
}

/// B3 毒值 `and` 的 `ones` 结果平面（逐字）。
pub open spec fn and_ones_b3(
    oa: Seq<u64>,
    xa: Option<Seq<u64>>,
    ob: Seq<u64>,
    xb: Option<Seq<u64>>,
    w: int,
) -> u64 {
    let x_out = unknown_word(xa, w) | unknown_word(xb, w);
    (oa[w] & ob[w]) & !x_out
}

/// B3 毒值 `and` 的 `unknowns` 结果平面（逐字）。
pub open spec fn and_unknowns_b3(
    oa: Seq<u64>,
    xa: Option<Seq<u64>>,
    ob: Seq<u64>,
    xb: Option<Seq<u64>>,
    w: int,
) -> u64 {
    unknown_word(xa, w) | unknown_word(xb, w)
}

/// B3 毒值 `or` 的 `ones` 结果平面（逐字）。
pub open spec fn or_ones_b3(
    oa: Seq<u64>,
    xa: Option<Seq<u64>>,
    ob: Seq<u64>,
    xb: Option<Seq<u64>>,
    w: int,
) -> u64 {
    let x_out = unknown_word(xa, w) | unknown_word(xb, w);
    (oa[w] | ob[w]) & !x_out
}

/// B3 毒值 `or` 的 `unknowns` 结果平面（逐字）。
pub open spec fn or_unknowns_b3(
    oa: Seq<u64>,
    xa: Option<Seq<u64>>,
    ob: Seq<u64>,
    xb: Option<Seq<u64>>,
    w: int,
) -> u64 {
    unknown_word(xa, w) | unknown_word(xb, w)
}

/// B3 毒值 `and`：逐字计算两个结果平面。
#[allow(unused_variables)]
pub fn and_planes_b3(
    len: usize,
    ones: &Vec<u64>,
    unknowns: &Option<Vec<u64>>,
    other_ones: &Vec<u64>,
    other_unknowns: &Option<Vec<u64>>,
) -> (r: (Vec<u64>, Vec<u64>))
    requires
        wf(len as nat, ones@, unknowns_view(unknowns)),
        wf(len as nat, other_ones@, unknowns_view(other_unknowns)),
    ensures
        r.0@ == Seq::new(
            words_for(len as nat),
            |w: int| and_ones_b3(
                ones@,
                unknowns_view(unknowns),
                other_ones@,
                unknowns_view(other_unknowns),
                w,
            ),
        ),
        r.1@ == Seq::new(
            words_for(len as nat),
            |w: int| and_unknowns_b3(
                ones@,
                unknowns_view(unknowns),
                other_ones@,
                unknowns_view(other_unknowns),
                w,
            ),
        ),
{
    let n = ones.len();
    let mut out_ones: Vec<u64> = Vec::new();
    let mut out_unknowns: Vec<u64> = Vec::new();
    for w in 0..n
        invariant
            ones@.len() == n,
            other_ones@.len() == n,
            n as nat == words_for(len as nat),
            match unknowns {
                Some(p) => p@.len() == n,
                None => true,
            },
            match other_unknowns {
                Some(p) => p@.len() == n,
                None => true,
            },
            out_ones@.len() == w,
            out_unknowns@.len() == w,
            forall|k: int| 0 <= k < w ==> #[trigger] out_ones@[k] == and_ones_b3(
                ones@,
                unknowns_view(unknowns),
                other_ones@,
                unknowns_view(other_unknowns),
                k,
            ),
            forall|k: int| 0 <= k < w ==> #[trigger] out_unknowns@[k] == and_unknowns_b3(
                ones@,
                unknowns_view(unknowns),
                other_ones@,
                unknowns_view(other_unknowns),
                k,
            ),
    {
        let o1 = ones[w];
        let o2 = other_ones[w];
        let x1: u64 = match unknowns {
            Some(p) => p[w],
            None => 0u64,
        };
        let x2: u64 = match other_unknowns {
            Some(p) => p[w],
            None => 0u64,
        };
        let x_out = x1 | x2;
        out_unknowns.push(x_out);
        out_ones.push((o1 & o2) & !x_out);
    }
    (out_ones, out_unknowns)
}

/// B3 毒值 `or`：逐字计算两个结果平面。
#[allow(unused_variables)]
pub fn or_planes_b3(
    len: usize,
    ones: &Vec<u64>,
    unknowns: &Option<Vec<u64>>,
    other_ones: &Vec<u64>,
    other_unknowns: &Option<Vec<u64>>,
) -> (r: (Vec<u64>, Vec<u64>))
    requires
        wf(len as nat, ones@, unknowns_view(unknowns)),
        wf(len as nat, other_ones@, unknowns_view(other_unknowns)),
    ensures
        r.0@ == Seq::new(
            words_for(len as nat),
            |w: int| or_ones_b3(
                ones@,
                unknowns_view(unknowns),
                other_ones@,
                unknowns_view(other_unknowns),
                w,
            ),
        ),
        r.1@ == Seq::new(
            words_for(len as nat),
            |w: int| or_unknowns_b3(
                ones@,
                unknowns_view(unknowns),
                other_ones@,
                unknowns_view(other_unknowns),
                w,
            ),
        ),
{
    let n = ones.len();
    let mut out_ones: Vec<u64> = Vec::new();
    let mut out_unknowns: Vec<u64> = Vec::new();
    for w in 0..n
        invariant
            ones@.len() == n,
            other_ones@.len() == n,
            n as nat == words_for(len as nat),
            match unknowns {
                Some(p) => p@.len() == n,
                None => true,
            },
            match other_unknowns {
                Some(p) => p@.len() == n,
                None => true,
            },
            out_ones@.len() == w,
            out_unknowns@.len() == w,
            forall|k: int| 0 <= k < w ==> #[trigger] out_ones@[k] == or_ones_b3(
                ones@,
                unknowns_view(unknowns),
                other_ones@,
                unknowns_view(other_unknowns),
                k,
            ),
            forall|k: int| 0 <= k < w ==> #[trigger] out_unknowns@[k] == or_unknowns_b3(
                ones@,
                unknowns_view(unknowns),
                other_ones@,
                unknowns_view(other_unknowns),
                k,
            ),
    {
        let o1 = ones[w];
        let o2 = other_ones[w];
        let x1: u64 = match unknowns {
            Some(p) => p[w],
            None => 0u64,
        };
        let x2: u64 = match other_unknowns {
            Some(p) => p[w],
            None => 0u64,
        };
        let x_out = x1 | x2;
        out_unknowns.push(x_out);
        out_ones.push((o1 | o2) & !x_out);
    }
    (out_ones, out_unknowns)
}

}
