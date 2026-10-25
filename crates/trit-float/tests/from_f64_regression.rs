//! 复审 H1 回归：低精度 `from_f64` 必须落在平衡尾数合法区间内，且取最近可表示值。

use trit_float::TritFloat;

#[test]
fn low_precision_picks_nearest_representable() {
    let cases = [
        (5.0, 1u32, 3.0),
        (-5.0, 1, -3.0),
        (1.5, 1, 1.0),
        (4.5, 1, 3.0),
        (2.0, 1, 3.0),
        (5.0, 2, 6.0),
        (0.5, 1, 1.0 / 3.0),
    ];
    for (value, precision, want) in cases {
        let got = TritFloat::from_f64(value, precision).unwrap().to_f64();
        assert!(
            (got - want).abs() < 1e-12,
            "from_f64({value}, {precision}) = {got}, want {want}"
        );
    }
}

#[test]
fn mantissa_never_exceeds_balance_limit() {
    for precision in 1..=8u32 {
        let limit = ((3f64.powi(precision as i32) - 1.0) / 2.0) as i128;
        for value in [0.5, 1.0, 1.5, 2.0, 4.5, 5.0, 13.5, 100.0, -7.0] {
            let f = TritFloat::from_f64(value, precision).unwrap();
            assert!(
                f.mantissa().abs() <= limit,
                "p={precision} v={value} mantissa={} limit={limit}",
                f.mantissa()
            );
        }
    }
}

#[test]
fn error_is_bounded_by_relative_bound() {
    for precision in 1..=6u32 {
        let bound = trit_float::relative_error_bound(precision);
        for value in [0.5, 0.7, 1.0, 1.3, 2.0, 5.0, 17.0, 123.456] {
            let got = TritFloat::from_f64(value, precision).unwrap().to_f64();
            let rel = ((got - value) / value).abs();
            assert!(
                rel <= bound + 1e-12,
                "p={precision} v={value} got={got} rel={rel} bound={bound}"
            );
        }
    }
}
