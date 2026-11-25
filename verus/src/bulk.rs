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
use crate::k3::Trit;

verus! {

mod proofs;

/// `len` 位需要的 64 位字数。
pub open spec fn words_for(len: nat) -> nat {
    (len + 63) / 64
}

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
        ones@.len() == words_for(len as nat),
        match unknowns {
            Some(p) => p@.len() == words_for(len as nat),
            None => true,
        },
        i < len,
    ensures
        r.0@.len() == words_for(len as nat),
        match r.1 {
            Some(p) => p@.len() == words_for(len as nat),
            None => true,
        },
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
    }
    (out_ones, r1)
}
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
    }
    (ones, unk)
}

}
