//! 两阶段扫描：全量等价 + 粗筛统计。

use trit_wspace::attribution::{AttributeOptions, EditMode, attribute};
use trit_wspace::scan::{ScanOptions, scan};
use trit_wspace::{Layer, Net, Oracle, Trit};

fn x0_net() -> Net {
    Net::new(vec![Layer::new(1, 2, &[1, 0], &[0]).expect("l")]).expect("net")
}

fn x0_oracle() -> Oracle {
    let inputs: Vec<Vec<Trit>> = (-1i8..=1)
        .flat_map(|a| (-1i8..=1).map(move |b| (a, b)))
        .map(|(a, b)| {
            vec![
                Trit::from_i8(a).expect("trit"),
                Trit::from_i8(b).expect("trit"),
            ]
        })
        .collect();
    let outputs = inputs.iter().map(|row| vec![row[0]]).collect();
    Oracle::new(inputs, outputs).expect("oracle")
}

#[test]
fn full_scan_matches_attribute() {
    let oracle = x0_oracle();
    let net = x0_net();
    let attr = attribute(
        &net,
        &oracle,
        &AttributeOptions {
            mode: EditMode::Flip,
            top: 0,
            ..AttributeOptions::default()
        },
    )
    .expect("attribute");
    let result = scan(
        &net,
        &oracle,
        &ScanOptions {
            mode: EditMode::Flip,
            top: 0,
            sample_budget: 0,
            ..ScanOptions::default()
        },
    )
    .expect("scan");
    assert!(!result.stats.sampled);
    assert_eq!(result.report.params, attr.params);
    assert_eq!(result.report.capabilities.len(), attr.capabilities.len());
    for (left, right) in result.report.capabilities.iter().zip(&attr.capabilities) {
        assert_eq!(left.lost, right.lost);
    }
}

#[test]
fn sampled_scan_reports_stats() {
    let oracle = x0_oracle();
    let net = x0_net();
    let result = scan(
        &net,
        &oracle,
        &ScanOptions {
            mode: EditMode::Flip,
            sample_budget: 3,
            top: 0,
            ..ScanOptions::default()
        },
    )
    .expect("scan");
    assert!(result.stats.sampled);
    assert!(result.stats.screened <= result.stats.total_params);
    assert!(result.stats.refined <= result.stats.screened);
    assert!(
        result
            .report
            .capabilities
            .iter()
            .any(|cap| !cap.lost.is_empty())
    );
}
