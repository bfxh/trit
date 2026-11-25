//! `table.rs`（编码真值表）与 `trit-core` 的 K3 实现逐项对拍。
//!
//! 三值是有限域（3 个值），`for a in ALL { for b in ALL }` 是完备穷举：
//! 不是抽样测试。验证链见 `refinement.rs` 与 `src/k3.rs`。

mod common;

use common::{code, decode};
use trit_core::value::Ternary;
use trit_verified::table;

#[test]
fn index_roundtrip_matches_table() {
    let indices: Vec<usize> = Ternary::ALL.iter().map(|t| t.index()).collect();
    assert_eq!(
        indices,
        vec![table::F as usize, table::U as usize, table::T as usize]
    );
    for c in table::ALL {
        assert_eq!(decode(c).index(), c as usize);
        assert_eq!(code(decode(c)), c);
    }
}

#[test]
fn k3_not_matches_trit_core() {
    for a in Ternary::ALL {
        assert_eq!(code(!a), table::not(code(a)), "!{a:?}");
    }
}

#[test]
fn k3_and_matches_trit_core() {
    for a in Ternary::ALL {
        for b in Ternary::ALL {
            assert_eq!(code(a & b), table::and(code(a), code(b)), "{a:?} & {b:?}");
        }
    }
}

#[test]
fn k3_or_matches_trit_core() {
    for a in Ternary::ALL {
        for b in Ternary::ALL {
            assert_eq!(code(a | b), table::or(code(a), code(b)), "{a:?} | {b:?}");
        }
    }
}

#[test]
fn k3_xor_matches_trit_core() {
    for a in Ternary::ALL {
        for b in Ternary::ALL {
            assert_eq!(code(a ^ b), table::xor(code(a), code(b)), "{a:?} ^ {b:?}");
        }
    }
}

#[test]
fn xor_is_not_eq_in_k3() {
    for a in table::ALL {
        for b in table::ALL {
            assert_eq!(table::xor(a, b), table::not(table::eq(a, b)), "{a}/{b}");
            assert_eq!(table::ne(a, b), table::not(table::eq(a, b)), "{a}/{b}");
        }
    }
}
