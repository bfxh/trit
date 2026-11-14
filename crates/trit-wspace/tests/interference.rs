//! 批量编辑干扰矩阵：矩阵对称/对角线为 0、贪心覆盖全部编辑、JSON 形状。

use serde_json::json;
use trit_wspace::interference::analyze_interference;
use trit_wspace::{Edit, Layer, Net, Oracle, Trit};

fn xor_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("xor net")
}

fn xor_oracle() -> Oracle {
    trit_wspace::wire::oracle_from_value(&json!({
        "domain": "boolean", "vars": 2, "table": "0110"
    }))
    .expect("oracle")
}

fn edits() -> Vec<Edit> {
    vec![
        Edit::SetTrit {
            layer: 0,
            row: 0,
            col: 0,
            to: Trit::Zero,
        },
        Edit::SetTrit {
            layer: 0,
            row: 0,
            col: 1,
            to: Trit::Zero,
        },
        Edit::SetTrit {
            layer: 1,
            row: 0,
            col: 0,
            to: Trit::Pos,
        },
        Edit::SetTrit {
            layer: 1,
            row: 0,
            col: 1,
            to: Trit::Zero,
        },
    ]
}

#[test]
fn matrix_is_symmetric_with_zero_diagonal() {
    let report = analyze_interference(&xor_net(), &xor_oracle(), &edits()).expect("analyze");
    let count = edits().len();
    assert_eq!(report.singles.len(), count);
    assert_eq!(report.gains.len(), count);
    assert_eq!(report.matrix.len(), count);
    for left in 0..count {
        assert_eq!(report.matrix[left].len(), count);
        assert_eq!(report.matrix[left][left], 0);
        for right in 0..count {
            assert_eq!(report.matrix[left][right], report.matrix[right][left]);
        }
    }
}

#[test]
fn greedy_covers_every_edit_once() {
    let report = analyze_interference(&xor_net(), &xor_oracle(), &edits()).expect("analyze");
    let mut seen = std::collections::BTreeSet::new();
    for step in &report.greedy {
        assert!(seen.insert(step.index), "编辑 {} 被选了两次", step.index);
    }
    assert_eq!(seen.len(), edits().len());
    assert_eq!(report.greedy.last().expect("last").loss, report.greedy_loss);
}

#[test]
fn gains_match_singles() {
    let report = analyze_interference(&xor_net(), &xor_oracle(), &edits()).expect("analyze");
    for (index, single) in report.singles.iter().enumerate() {
        assert_eq!(
            report.gains[index],
            report.base_loss as isize - *single as isize
        );
    }
}

#[test]
fn empty_edit_list_is_an_error() {
    let error = analyze_interference(&xor_net(), &xor_oracle(), &[]).expect_err("必须报错");
    assert!(format!("{error}").contains("至少一条编辑"), "{error}");
}

#[test]
fn json_has_all_fields() {
    let report = analyze_interference(&xor_net(), &xor_oracle(), &edits()).expect("analyze");
    let value = report.to_json();
    assert!(value["base_loss"].is_u64());
    assert_eq!(
        value["matrix"].as_array().expect("matrix").len(),
        edits().len()
    );
    assert_eq!(
        value["greedy"].as_array().expect("greedy").len(),
        edits().len()
    );
    assert!(value["greedy_loss"].is_u64());
}
