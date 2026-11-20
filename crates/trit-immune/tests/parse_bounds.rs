//! 22–36 轮解析型 panic 的回归（第 23 轮：快照 `hist_len` 可让 `Vec::with_capacity` 溢出）。

use trit_immune::snapshot;

#[test]
fn absurd_hist_len_is_rejected_without_allocating() {
    let text = format!(
        "{}\nCOVERED\t0\t0\t0000000000000000\nNODE\t0\t-\tdev\troot\t0\t0\t{}\n",
        snapshot::SNAPSHOT_HEADER,
        usize::MAX
    );
    assert!(
        snapshot::parse(&text).is_err(),
        "hist_len 超过剩余行数必须被拒（不预分配）"
    );
}
