//! `GroupedLayer::sampled_oracle`：接进本仓现有验证流程的采样通道。

use trit_wspace::grouped::GroupedLayer;
use trit_wspace::grouped_oracle::MAX_SAMPLES;
use trit_wspace::trit::Trit;

fn layer() -> GroupedLayer {
    GroupedLayer::from_trits(
        2,
        3,
        &[1, 0, -1, 1, 1, -1],
        &[0, 1, 0, 1, 0, 1],
        &[0.5, 2.0],
        &[0.0, 0.0],
    )
    .expect("layer")
}

#[test]
fn sampled_oracle_has_expected_size() {
    let oracle = layer().sampled_oracle(3, 16, 42).expect("oracle");
    assert_eq!(oracle.len(), 16, "样本数应等于 samples");
    assert_eq!(oracle.inputs()[0].len(), 3, "输入维数应等于 vars");
    assert_eq!(oracle.outputs()[0].len(), 2, "输出维数应等于层行数");
}

#[test]
fn sampled_oracle_is_deterministic() {
    let a = layer().sampled_oracle(3, 8, 7).expect("a");
    let b = layer().sampled_oracle(3, 8, 7).expect("b");
    assert_eq!(a.inputs(), b.inputs(), "同 seed 输入必须相同");
    assert_eq!(a.outputs(), b.outputs(), "同 seed 输出必须相同");

    let c = layer().sampled_oracle(3, 8, 8).expect("c");
    assert_ne!(a.inputs(), c.inputs(), "换 seed 应得到不同输入");
}

#[test]
fn sampled_outputs_match_direct_forward() {
    let layer = layer();
    let oracle = layer.sampled_oracle(3, 12, 99).expect("oracle");
    for (input, expected) in oracle.inputs().iter().zip(oracle.outputs()) {
        let features: Vec<f32> = input.iter().map(|t| f32::from(t.to_i8())).collect();
        let out = layer.forward(&features).expect("forward");
        let sign: Vec<Trit> = out
            .iter()
            .map(|v| {
                if *v > 1e-9 {
                    Trit::Pos
                } else if *v < -1e-9 {
                    Trit::Neg
                } else {
                    Trit::Zero
                }
            })
            .collect();
        assert_eq!(&sign, expected, "oracle 输出应与直接 forward 的符号一致");
    }
}

#[test]
fn sampled_oracle_rejects_bad_args() {
    let layer = layer();
    assert!(layer.sampled_oracle(0, 4, 1).is_err(), "vars=0 应报错");
    assert!(layer.sampled_oracle(4, 4, 1).is_err(), "vars>cols 应报错");
    assert!(layer.sampled_oracle(3, 0, 1).is_err(), "samples=0 应报错");
    assert!(
        layer.sampled_oracle(3, MAX_SAMPLES + 1, 1).is_err(),
        "samples 超上限应报错"
    );
}

#[test]
fn sampled_oracle_uses_full_ternary_alphabet() {
    let oracle = layer().sampled_oracle(3, 200, 2024).expect("oracle");
    let mut seen = [false; 3];
    for input in oracle.inputs() {
        for trit in input {
            seen[match trit {
                Trit::Neg => 0,
                Trit::Zero => 1,
                Trit::Pos => 2,
            }] = true;
        }
    }
    assert!(seen.iter().all(|hit| *hit), "200 个样本应覆盖 -1/0/+1 三值");
}
