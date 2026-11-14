//! 分位口径：线性插值（与 NumPy 默认一致），不是最近秩。

use trit_flow::{percentile, summarize};

#[test]
fn percentile_linear_interpolation() {
    let v = [0.0, 10.0];
    assert!((percentile(&v, 0.5) - 5.0).abs() < 1e-12);
    assert!((percentile(&v, 0.0)).abs() < 1e-12);
    assert!((percentile(&v, 1.0) - 10.0).abs() < 1e-12);

    let v = [0.0, 10.0, 20.0];
    assert!((percentile(&v, 0.25) - 5.0).abs() < 1e-12);
    assert!((percentile(&v, 0.75) - 15.0).abs() < 1e-12);

    let one = [7.0];
    assert!((percentile(&one, 0.99) - 7.0).abs() < 1e-12);
}

#[test]
fn summarize_reports_extremes_and_mean() {
    let samples = [1.0, 2.0, 3.0, 4.0, 100.0];
    let r = summarize(&samples);
    assert_eq!(r.n, 5);
    assert!((r.min - 1.0).abs() < 1e-12);
    assert!((r.max - 100.0).abs() < 1e-12);
    assert!((r.p50 - 3.0).abs() < 1e-12);
    assert!((r.mean - 22.0).abs() < 1e-12);
    // p99 一定 ≥ p90 ≥ p50
    assert!(r.p99 >= r.p90 && r.p90 >= r.p50);
}

#[test]
fn summarize_does_not_mutate_input() {
    let samples = [3.0, 1.0, 2.0];
    let _ = summarize(&samples);
    assert_eq!(samples, [3.0, 1.0, 2.0], "读数不应改调用方切片");
}
