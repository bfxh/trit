//! #591：计数超过 `u128` 时必须饱和并如实报告，而不是 debug panic / release 回绕。

use trit_count::cnf::Cnf;
use trit_count::dpll::count_cnf;

#[test]
fn free_variable_count_saturates_instead_of_panicking() {
    // 200 个自由变量：2^200 超出 u128。
    let cnf = Cnf {
        nvars: 200,
        clauses: Vec::new(),
    };
    let result = count_cnf(&cnf);
    assert_eq!(result.count, u128::MAX);
    assert!(result.stats.overflows > 0);
}

#[test]
fn ordinary_counts_do_not_report_overflow() {
    let cnf = Cnf {
        nvars: 8,
        clauses: Vec::new(),
    };
    let result = count_cnf(&cnf);
    assert_eq!(result.count, 1 << 8);
    assert_eq!(result.stats.overflows, 0);
}
