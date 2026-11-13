//! 组件清单与单参数能力归属的测试。

use trit_wspace::attribution::{AttributeOptions, EditMode, attribute};
use trit_wspace::inventory::inventory;
use trit_wspace::{Layer, Net, Oracle, Trit};

fn xor_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("net")
}

#[test]
fn counts_layers_neurons_and_trits() {
    let inv = inventory(&xor_net());
    assert_eq!(inv.arch, vec![2, 2, 1]);
    assert_eq!(inv.layers.len(), 2);
    assert_eq!(inv.layers[0].rows, 2);
    assert_eq!(inv.layers[0].cols, 2);
    assert_eq!(inv.layers[0].trits, 4);
    assert_eq!(inv.layers[1].rows, 1);
    assert_eq!(inv.layers[1].cols, 2);
    assert_eq!(inv.layers[1].trits, 2);
    assert_eq!(inv.neurons, 3);
    assert_eq!(inv.trits, 6);
    assert_eq!(inv.bias, 3);
}

#[test]
fn flags_dead_rows_and_bias() {
    let net = Net::new(vec![Layer::new(2, 2, &[0, 0, 1, -1], &[0, 0]).expect("l")]).expect("net");
    let inv = inventory(&net);
    assert_eq!(inv.zero_rows, 1);
    assert_eq!(inv.layers[0].zero_rows, 1);
    assert_eq!(inv.layers[0].bias_nonzero, 0);
}

#[test]
fn attributes_one_trit_to_samples() {
    let oracle = x0_oracle();
    let net = Net::new(vec![Layer::new(1, 2, &[1, 0], &[0]).expect("l")]).expect("net");
    let options = AttributeOptions {
        mode: EditMode::Flip,
        top: 0,
        ..AttributeOptions::default()
    };
    let report = attribute(&net, &oracle, &options).expect("attribute");
    assert_eq!(report.samples, 9);
    assert_eq!(report.params, 3);
    let col0 = report
        .top
        .iter()
        .find(|a| a.param.col == Some(0))
        .expect("col0");
    let col1 = report
        .top
        .iter()
        .find(|a| a.param.col == Some(1))
        .expect("col1");
    assert_eq!(col0.lost.len(), 6, "x0 非零的 6 个样本都会翻转");
    assert_eq!(col1.lost.len(), 4, "col1 影响 4 个样本：x0=0 的两个 + x0 与 x1 相反的样本");
    assert_eq!(col0.specificity, 1.0);
    assert_eq!(col1.specificity, 1.0);
    assert_eq!(report.capabilities.len(), 3);
    assert!(
        report.specialists.iter().any(|p| p.col == Some(1)),
        "col1 只影响 3 个样本，应被标为专才"
    );
    assert!(!col0.sole.is_empty(), "col0 是它那组能力的唯一支撑");
}

fn x0_oracle() -> Oracle {
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    for a in [-1i8, 0, 1] {
        for b in [-1i8, 0, 1] {
            let row = vec![
                Trit::from_i8(a).expect("trit"),
                Trit::from_i8(b).expect("trit"),
            ];
            outputs.push(vec![row[0]]);
            inputs.push(row);
        }
    }
    Oracle::new(inputs, outputs).expect("oracle")
}
