//! f32 差分 → 三值补丁：trim、形状校验、应用后的三值一致性。

use trit_wspace::delta;
use trit_wspace::safetensors::Weights;
use trit_wspace::{Net, Trit};

fn base() -> Weights {
    Weights {
        rows: 2,
        cols: 3,
        values: vec![1.0, 1.0, 1.0, 1.0, 1.0, 1.0],
    }
}

fn tuned() -> Weights {
    Weights {
        rows: 2,
        cols: 3,
        values: vec![-1.0, -1.0, 1.0, 1.0, 1.0, 1.0],
    }
}

fn trit(net: &Net, row: usize, col: usize) -> Trit {
    net.layers()[0].trit(row, col).expect("trit")
}

#[test]
fn full_diff_matches_tuned_trits_and_bias() {
    let base_bias = vec![1.0, 1.0];
    let tuned_bias = vec![-1.0, -1.0];
    let (patch, base_net, stats) =
        delta::diff_weights(&base(), &tuned(), Some(&base_bias), Some(&tuned_bias), 1.0)
            .expect("diff");
    assert_eq!(stats.changed_trits, 2);
    assert_eq!(stats.changed_bias, 2);
    assert_eq!(stats.kept(), 4);
    assert_eq!(patch.len(), 4);

    let mut net = base_net.clone();
    patch.apply(&mut net).expect("apply");
    assert_eq!(trit(&net, 0, 0), Trit::Neg);
    assert_eq!(trit(&net, 0, 1), Trit::Neg);
    assert_eq!(trit(&net, 0, 2), Trit::Pos);
    assert_eq!(net.layers()[0].bias_trit(0), Some(Trit::Neg));
    assert_eq!(net.layers()[0].bias_trit(1), Some(Trit::Neg));
}

#[test]
fn trim_keeps_top_importance() {
    let base_bias = vec![1.0, 1.0];
    let tuned_bias = vec![-1.0, -1.0];
    let (patch, _, stats) =
        delta::diff_weights(&base(), &tuned(), Some(&base_bias), Some(&tuned_bias), 0.5)
            .expect("diff");
    assert_eq!(stats.changed(), 4);
    assert_eq!(stats.kept(), 2);
    assert_eq!(patch.len(), 2);
}

#[test]
fn rejects_shape_mismatch() {
    let wrong = Weights {
        rows: 2,
        cols: 2,
        values: vec![1.0; 4],
    };
    assert!(delta::diff_weights(&base(), &wrong, None, None, 1.0).is_err());
}

#[test]
fn no_change_is_empty_patch() {
    let (patch, _, stats) = delta::diff_weights(&base(), &base(), None, None, 1.0).expect("diff");
    assert!(patch.is_empty());
    assert_eq!(stats.changed(), 0);
}
