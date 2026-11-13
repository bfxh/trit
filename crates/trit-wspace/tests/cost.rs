//! 计算量与扰动的测试。

use trit_wspace::cost::cost;
use trit_wspace::{Layer, Net, Oracle};

fn xor_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("net")
}

#[test]
fn cost_counts_multiplies_per_layer() {
    let net = xor_net();
    let oracle = Oracle::boolean(2, |b| b[0] != b[1]).expect("oracle");
    let report = cost(&net, &oracle, 2).expect("cost");
    assert_eq!(report.samples, 4);
    assert_eq!(report.per_layer.len(), 2);
    assert!(report.total > 0);
    assert!(report.max >= report.min);
    assert!(report.top.len() <= 2);
    for sample in &report.top {
        let sum: usize = sample.per_layer.iter().map(|layer| layer.multiplies).sum();
        assert_eq!(sample.multiplies, sum);
    }
}
