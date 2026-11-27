//! F3 回归：[`TriScore::state`] 对 NaN 毒值走显式无效输入分支，不落入"最大值胜出"。

use trit_core::value::Ternary;
use trit_graph::TriScore;

#[test]
fn three_branches_and_tie_are_decided_as_before() {
    assert_eq!(
        TriScore {
            plus: 1.0,
            minus: 0.0,
            unknown: 0.0
        }
        .state(),
        Ternary::True
    );
    assert_eq!(
        TriScore {
            plus: 0.0,
            minus: 1.0,
            unknown: 0.0
        }
        .state(),
        Ternary::False
    );
    assert_eq!(
        TriScore {
            plus: 1.0,
            minus: 1.0,
            unknown: 0.0
        }
        .state(),
        Ternary::Unknown
    );
    assert_eq!(
        TriScore {
            plus: 0.0,
            minus: 0.0,
            unknown: 0.0
        }
        .state(),
        Ternary::Unknown
    );
}

#[test]
fn nan_channels_route_to_invalid_branch() {
    let nan = f64::NAN;
    let cases = [
        TriScore {
            plus: nan,
            minus: 1.0,
            unknown: 0.0,
        },
        TriScore {
            plus: 1.0,
            minus: nan,
            unknown: 0.0,
        },
        TriScore {
            plus: 1.0,
            minus: 0.0,
            unknown: nan,
        },
        TriScore {
            plus: nan,
            minus: nan,
            unknown: nan,
        },
    ];
    for score in cases {
        assert_eq!(score.state(), Ternary::Unknown);
    }
}

#[test]
fn nan_unknown_does_not_let_plus_win() {
    // 修前：unknown=NaN 被忽略，plus 1.0 胜出判 True（毒值错判）。
    let score = TriScore {
        plus: 1.0,
        minus: 0.0,
        unknown: f64::NAN,
    };
    assert_ne!(score.state(), Ternary::True);
    assert_eq!(score.state(), Ternary::Unknown);
}

#[test]
fn inf_still_behaves_as_before() {
    assert_eq!(
        TriScore {
            plus: f64::INFINITY,
            minus: 1.0,
            unknown: 0.0
        }
        .state(),
        Ternary::True
    );
    assert_eq!(
        TriScore {
            plus: f64::INFINITY,
            minus: f64::INFINITY,
            unknown: 0.0
        }
        .state(),
        Ternary::Unknown
    );
}
