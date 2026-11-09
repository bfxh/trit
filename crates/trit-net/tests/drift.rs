//! 漂移门的测试：绿路径、三个方向的变红、fail-closed（缺 band / 多 band /
//! 坏 JSON / 阈值非法）。全部纯函数，无网络无文件。

use trit_net::netsec::drift::{
    BandReading, CalibrationReport, DriftLimits, drift_check, parse_report,
};

fn report(entries: &[(f64, f64, f64, f64)]) -> CalibrationReport {
    CalibrationReport {
        bands: entries
            .iter()
            .map(|(band, fpr, tpr, abstain)| BandReading {
                band: *band,
                fpr: *fpr,
                tpr: *tpr,
                abstain: *abstain,
                feature_hist: None,
            })
            .collect(),
    }
}

/// 一条带逐特征读数的测试输入：`(band, fpr, tpr, abstain, hist)`。
type HistEntry = (f64, f64, f64, f64, Vec<[f64; 3]>);

/// 带逐特征三值分布的报告（`hist[feature] = [F, X, T]`）。
fn report_with_hist(entries: &[HistEntry]) -> CalibrationReport {
    CalibrationReport {
        bands: entries
            .iter()
            .map(|(band, fpr, tpr, abstain, hist)| BandReading {
                band: *band,
                fpr: *fpr,
                tpr: *tpr,
                abstain: *abstain,
                feature_hist: Some(hist.clone()),
            })
            .collect(),
    }
}

#[test]
fn identical_readings_are_green() {
    let base = report(&[(0.0, 0.14, 0.88, 0.0), (0.2, 0.14, 0.94, 0.14)]);
    let verdict = drift_check(&base, &base, DriftLimits::default());
    assert_eq!(verdict.verdict, "green");
    assert!(verdict.structural.is_empty());
    assert!(verdict.bands.iter().all(|band| band.reasons.is_empty()));
    // Δ 全为零。
    assert!(
        verdict
            .bands
            .iter()
            .all(|band| band.dfpr == 0.0 && band.dtpr == 0.0 && band.dabstain == 0.0)
    );
}

#[test]
fn small_drift_within_limits_stays_green() {
    let base = report(&[(0.2, 0.14, 0.94, 0.14)]);
    let current = report(&[(0.2, 0.20, 0.90, 0.30)]);
    // FPR +0.06 ≤ 0.1、TPR −0.04 ≤ 0.1、弃权 +0.16 ≤ 0.2 ⇒ 仍在容差内。
    let verdict = drift_check(&base, &current, DriftLimits::default());
    assert_eq!(verdict.verdict, "green", "{verdict:?}");
}

#[test]
fn fpr_rise_turns_red() {
    let base = report(&[(0.0, 0.14, 0.88, 0.0)]);
    let current = report(&[(0.0, 0.30, 0.88, 0.0)]);
    let verdict = drift_check(&base, &current, DriftLimits::default());
    assert_eq!(verdict.verdict, "red");
    assert!(verdict.bands[0].reasons[0].contains("FPR"), "{verdict:?}");
}

#[test]
fn tpr_drop_turns_red() {
    let base = report(&[(0.0, 0.14, 0.94, 0.0)]);
    let current = report(&[(0.0, 0.14, 0.70, 0.0)]);
    let verdict = drift_check(&base, &current, DriftLimits::default());
    assert_eq!(verdict.verdict, "red");
    assert!(verdict.bands[0].reasons[0].contains("TPR"), "{verdict:?}");
}

#[test]
fn abstain_surge_turns_red() {
    let base = report(&[(0.0, 0.14, 0.88, 0.10)]);
    let current = report(&[(0.0, 0.14, 0.88, 0.50)]);
    let verdict = drift_check(&base, &current, DriftLimits::default());
    assert_eq!(verdict.verdict, "red");
    assert!(verdict.bands[0].reasons[0].contains("弃权"), "{verdict:?}");
}

#[test]
fn missing_band_fails_closed() {
    let base = report(&[(0.0, 0.14, 0.88, 0.0), (0.2, 0.14, 0.94, 0.14)]);
    let current = report(&[(0.0, 0.14, 0.88, 0.0)]);
    let verdict = drift_check(&base, &current, DriftLimits::default());
    assert_eq!(verdict.verdict, "red");
    assert!(verdict.structural[0].contains("缺失"), "{verdict:?}");
}

#[test]
fn extra_band_is_config_drift_and_red() {
    let base = report(&[(0.0, 0.14, 0.88, 0.0)]);
    let current = report(&[(0.0, 0.14, 0.88, 0.0), (0.5, 0.10, 0.99, 0.0)]);
    let verdict = drift_check(&base, &current, DriftLimits::default());
    assert_eq!(verdict.verdict, "red");
    assert!(verdict.structural[0].contains("没有的 band"), "{verdict:?}");
}

#[test]
fn negative_limits_are_refused() {
    let base = report(&[(0.0, 0.14, 0.88, 0.0)]);
    let verdict = drift_check(
        &base,
        &base,
        DriftLimits {
            max_fpr_delta: -0.1,
            ..DriftLimits::default()
        },
    );
    assert_eq!(verdict.verdict, "red");
    assert!(verdict.structural[0].contains("阈值"), "{verdict:?}");
    // NaN 阈值同理。
    let verdict = drift_check(
        &base,
        &base,
        DriftLimits {
            max_tpr_delta: f64::NAN,
            ..DriftLimits::default()
        },
    );
    assert_eq!(verdict.verdict, "red");
}

#[test]
fn parse_rejects_bad_json_and_non_finite_readings() {
    assert!(parse_report("not json").is_err());
    assert!(parse_report("{}").is_err(), "缺 bands 数组必须报错");
    // 读数含非有限值：JSON 里写不成 NaN，但字符串数字/缺字段会走 serde 错误；
    // 显式构造非有限值的路径由门本身对 fpr/tpr/abstain 的有限性检查兜底。
    let bad = r#"{"bands":[{"band":0.0,"fpr":"oops","tpr":0.9,"abstain":0.0}]}"#;
    assert!(parse_report(bad).is_err());
    let good = r#"{"bands":[{"band":0.0,"fpr":0.14,"tpr":0.88,"abstain":0.0}],
                    "epochs":300,"hidden":16}"#;
    let report = parse_report(good).expect("多余字段应被忽略（向前兼容）");
    assert_eq!(report.bands.len(), 1);
}

#[test]
fn mixed_reasons_accumulate_per_band() {
    // 一个 band 同时触两条理由，另一个 band 绿。
    let base = report(&[(0.0, 0.10, 0.95, 0.0), (0.2, 0.10, 0.95, 0.0)]);
    let current = report(&[(0.0, 0.30, 0.70, 0.0), (0.2, 0.10, 0.95, 0.0)]);
    let verdict = drift_check(&base, &current, DriftLimits::default());
    assert_eq!(verdict.verdict, "red");
    assert_eq!(verdict.bands[0].reasons.len(), 2, "{verdict:?}");
    assert!(verdict.bands[1].reasons.is_empty());
}

// ---- 逐特征归因 ----

fn hist(rows: &[[f64; 3]]) -> Vec<[f64; 3]> {
    rows.to_vec()
}

#[test]
fn feature_shift_turns_red() {
    // 特征 0 的 T 态占比 0.20 → 0.55（Δ +0.35 > 0.1）⇒ 红，且理由指向特征 0。
    let base = report_with_hist(&[(0.0, 0.10, 0.90, 0.0, hist(&[
        [0.30, 0.50, 0.20],
        [0.20, 0.30, 0.50],
    ]))]);
    let current = report_with_hist(&[(0.0, 0.10, 0.90, 0.0, hist(&[
        [0.05, 0.40, 0.55],
        [0.20, 0.30, 0.50],
    ]))]);
    let verdict = drift_check(&base, &current, DriftLimits::default());
    assert_eq!(verdict.verdict, "red");
    assert_eq!(verdict.feature_attribution, "ok");
    let shifts = &verdict.bands[0].feature_shifts;
    assert!(shifts.iter().any(|s| s.feature == 0 && s.state == "T"), "{verdict:?}");
    assert!(verdict.bands[0].reasons.iter().any(|r| r.contains("特征 #0")), "{verdict:?}");
}

#[test]
fn feature_shift_within_limit_stays_green() {
    // 三态占比各动 ≤0.05（< 0.1）⇒ 仍绿，但归因可用。
    let base = report_with_hist(&[(0.0, 0.10, 0.90, 0.0, hist(&[[0.30, 0.40, 0.30]]))]);
    let current = report_with_hist(&[(0.0, 0.10, 0.90, 0.0, hist(&[[0.33, 0.40, 0.27]]))]);
    let verdict = drift_check(&base, &current, DriftLimits::default());
    assert_eq!(verdict.verdict, "green", "{verdict:?}");
    assert_eq!(verdict.feature_attribution, "ok");
    assert!(verdict.bands[0].feature_shifts.is_empty());
}

#[test]
fn missing_feature_hist_marks_unavailable_without_red() {
    // 旧基线没有 feature_hist：只做 band 级对账，读数一致 ⇒ 绿，但归因标记不可用。
    let base = report(&[(0.0, 0.10, 0.90, 0.0)]);
    let verdict = drift_check(&base, &base, DriftLimits::default());
    assert_eq!(verdict.verdict, "green");
    assert_eq!(verdict.feature_attribution, "unavailable");
    assert!(verdict.bands[0].feature_shifts.is_empty());
}

#[test]
fn require_feature_hist_turns_red() {
    let base = report(&[(0.0, 0.10, 0.90, 0.0)]);
    let verdict = drift_check(
        &base,
        &base,
        DriftLimits {
            require_feature_hist: true,
            ..DriftLimits::default()
        },
    );
    assert_eq!(verdict.verdict, "red");
    assert!(
        verdict.structural.iter().any(|r| r.contains("逐特征读数")),
        "{verdict:?}"
    );
}

#[test]
fn feature_dim_mismatch_fails_closed() {
    // 基线 2 维、当前 1 维 ⇒ 结构红（不能悄悄只比第一个）。
    let base = report_with_hist(&[(0.0, 0.10, 0.90, 0.0, hist(&[[0.3, 0.4, 0.3], [0.2, 0.3, 0.5]]))]);
    let current = report_with_hist(&[(0.0, 0.10, 0.90, 0.0, hist(&[[0.3, 0.4, 0.3]]))]);
    let verdict = drift_check(&base, &current, DriftLimits::default());
    assert_eq!(verdict.verdict, "red");
    assert!(
        verdict.structural.iter().any(|r| r.contains("维数不一致")),
        "{verdict:?}"
    );
}

#[test]
fn feature_delta_threshold_is_configurable() {
    // Δ +0.15：默认阈值 0.1 判红；放宽到 0.2 则绿。
    let base = report_with_hist(&[(0.0, 0.10, 0.90, 0.0, hist(&[[0.30, 0.40, 0.30]]))]);
    let current = report_with_hist(&[(0.0, 0.10, 0.90, 0.0, hist(&[[0.15, 0.40, 0.45]]))]);
    let strict = drift_check(&base, &current, DriftLimits::default());
    assert_eq!(strict.verdict, "red");
    let loose = drift_check(
        &base,
        &current,
        DriftLimits {
            max_feature_delta: 0.2,
            ..DriftLimits::default()
        },
    );
    assert_eq!(loose.verdict, "green", "{loose:?}");
}
