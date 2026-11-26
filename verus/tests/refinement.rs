//! `k3.rs` 的 Verus exec 提炼与 `table.rs` 对拍。
//!
//! 配合 `src/k3.rs` 的 `ensures`（spec ≡ exec）与 `bridge.rs`（table ≡ trit-core），
//! 得到 `trit-core ≡ table ≡ exec ≡ spec`。

mod common;

use common::{trit, trit_code};
use trit_verified::k3::{knot_exec, tand_exec, teq_exec, tor_exec};
use trit_verified::table;

#[test]
fn exec_refinement_matches_table() {
    for a in table::ALL {
        assert_eq!(trit_code(knot_exec(trit(a))), table::not(a), "!{a}");
        for b in table::ALL {
            assert_eq!(
                trit_code(tand_exec(trit(a), trit(b))),
                table::and(a, b),
                "{a} & {b}"
            );
            assert_eq!(
                trit_code(tor_exec(trit(a), trit(b))),
                table::or(a, b),
                "{a} | {b}"
            );
            assert_eq!(
                trit_code(teq_exec(trit(a), trit(b))),
                table::eq(a, b),
                "{a} == {b}"
            );
        }
    }
}
