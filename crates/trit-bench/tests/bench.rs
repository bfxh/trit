//! trit-bench 的判据测试：确定性、分布、分位数口径、轮内顺序轮换。

use std::cell::RefCell;
use std::rc::Rc;

use trit_bench::{Bench, SeedRng, Spread};

#[test]
fn same_seed_reproduces_the_same_vectors() {
    let a: Vec<Vec<i32>> = (0..3).map(|_| SeedRng::new(0xD1E5).trits(64)).collect();
    for row in &a {
        assert_eq!(
            row, &a[0],
            "同种子必须逐位相同（这是替代 8 份手写 RNG 的前提）"
        );
    }
    let b = SeedRng::new(0xCAFE).trits(64);
    assert_ne!(b, a[0], "不同种子应当给出不同序列");
}

#[test]
fn trits_cover_the_three_values_and_stay_balanced() {
    let values = SeedRng::new(0x1234_5678).trits(30_000);
    let mut counts = [0usize; 3];
    for &value in &values {
        assert!(
            (-1..=1).contains(&value),
            "值域必须是 {{-1,0,+1}}，拿到 {value}"
        );
        counts[(value + 1) as usize] += 1;
    }
    for (value, count) in (-1i32..=1).zip(counts) {
        assert!(
            count > 8_000 && count < 12_000,
            "值 {value} 出现 {count} 次，偏离均衡"
        );
    }
}

#[test]
fn seed_zero_does_not_degenerate() {
    let values = SeedRng::new(0).trits(64);
    let distinct: std::collections::BTreeSet<i32> = values.iter().copied().collect();
    assert!(
        distinct.len() > 1,
        "种子 0 会卡在 xorshift 的不动点 ⇒ 整段同一个值（实测退化成全 -1，不是全 0）"
    );
}

#[test]
fn spread_reports_min_median_max() {
    let spread = Spread::from_samples(&[5.0, 1.0, 9.0, 3.0, 7.0]);
    assert_eq!(spread.min_ns, 1.0);
    assert_eq!(spread.median_ns, 5.0);
    assert_eq!(spread.max_ns, 9.0);

    let even = Spread::from_samples(&[4.0, 1.0, 2.0, 3.0]);
    assert_eq!(even.min_ns, 1.0);
    assert_eq!(
        even.median_ns, 3.0,
        "偶数样本取上中位（与既有读数口径一致）"
    );

    let empty = Spread::from_samples(&[]);
    assert_eq!(
        (empty.min_ns, empty.median_ns, empty.max_ns),
        (0.0, 0.0, 0.0)
    );
}

#[test]
fn per_unit_conversions_use_the_median() {
    let spread = Spread {
        min_ns: 100.0,
        median_ns: 200.0,
        max_ns: 900.0,
    };
    assert_eq!(spread.ns_per_unit(4), 50.0);
    assert_eq!(spread.ps_per_unit(4), 50_000.0, "ns → ps 是 1000 倍");
}

#[test]
fn rounds_interleave_and_rotate_the_starting_order() {
    let order = Rc::new(RefCell::new(Vec::new()));
    let mut bench = Bench::new(1);
    for label in ["a", "b", "c"] {
        let order = Rc::clone(&order);
        let name = label.to_string();
        bench.add(label, move || {
            order.borrow_mut().push(name.clone());
            0
        });
    }
    let samples = bench.run(4);
    let seen = order.borrow();
    assert_eq!(seen.len(), 12, "4 轮 × 3 实现 = 12 次调用");
    assert_eq!(
        &seen[..6],
        &["a", "b", "c", "b", "c", "a"],
        "必须轮内交错，且每轮从不同位序起步（否则靠后的实现总吃到热机位）"
    );
    assert!(
        samples.iter().all(|round| round.len() == 4),
        "每个实现都要有 4 个样本"
    );
}

#[test]
fn measure_aligns_labels_with_samples() {
    let mut bench = Bench::new(10);
    bench.add("one", || 1);
    bench.add("two", || 2);
    let report = bench.measure(3);
    let labels: Vec<&str> = report.iter().map(|(label, _)| label.as_str()).collect();
    assert_eq!(labels, ["one", "two"], "读数必须按登记顺序对齐，不能被打乱");
    assert!(
        report
            .iter()
            .all(|(_, spread)| spread.max_ns >= spread.min_ns)
    );
    assert_eq!(bench.units(), 10);
}

#[test]
fn reps_multiply_the_work_and_average_the_reading() {
    let calls = Rc::new(RefCell::new(0usize));
    let mut bench = Bench::new(10).reps(5);
    let counter = Rc::clone(&calls);
    bench.add("sink", move || {
        *counter.borrow_mut() += 1;
        1
    });
    let samples = bench.run(3);
    assert_eq!(
        *calls.borrow(),
        15,
        "3 轮 × 5 次重复 = 15 次调用（读数取平均才是这个意思）"
    );
    assert_eq!(samples[0].len(), 3);
}

#[test]
fn zero_or_missing_reps_stays_safe() {
    let mut bench = Bench::new(1).reps(0);
    bench.add("one", || 7);
    assert_eq!(
        bench.run(2)[0].len(),
        2,
        "reps(0) 必须按 1 处理，不能一圈不跑"
    );
}

/// `below` 的三条：确定性、上界闭合、空域直接判红（不静默返回 0）。
#[test]
fn below_is_deterministic_and_bounded() {
    let seq_a: Vec<usize> = {
        let mut rng = SeedRng::new(21);
        (0..200).map(|_| rng.below(7)).collect()
    };
    let seq_b: Vec<usize> = {
        let mut rng = SeedRng::new(21);
        (0..200).map(|_| rng.below(7)).collect()
    };
    assert_eq!(seq_a, seq_b, "同种子必须给同序列");
    assert!(
        seq_a.iter().all(|&v| v < 7),
        "越界：{:?}",
        seq_a.iter().find(|&&v| v >= 7)
    );
    assert!(seq_a.iter().any(|&v| v == 0), "从没落到 0，像是分布坏了");

    let r = std::panic::catch_unwind(|| SeedRng::new(1).below(0));
    assert!(r.is_err(), "below(0) 该 panic，而不是回 0");
}
