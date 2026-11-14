//! 训练过程/梯度历史的测试。

use trit_wspace::repair::RepairOptions;
use trit_wspace::training::run;
use trit_wspace::{Edit, Layer, Net, Oracle, Patch, Trit};

fn xor_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("net")
}

fn xor_oracle() -> Oracle {
    Oracle::boolean(2, |b| b[0] != b[1]).expect("oracle")
}

#[test]
fn records_loss_and_sensitivity_per_round() {
    let mut net = xor_net();
    Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Zero,
    }])
    .apply(&mut net)
    .expect("break");
    let history = run(&mut net, &xor_oracle(), 2, &RepairOptions::default()).expect("run");
    assert!(!history.rounds.is_empty());
    let first = &history.rounds[0];
    assert!(first.before_loss > 0);
    assert_eq!(first.after_loss, 0);
    assert!(first.patch_len > 0);
    assert!(first.flip_rate >= 0.0);
    assert!(first.lost_rate >= 0.0);
    assert!(first.max_position_flips > 0);
}
