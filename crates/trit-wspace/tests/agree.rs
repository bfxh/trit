//! `agree` 模块的性质测试：f32 投影、完整前向、体积与三值化边界。

use trit_wspace::agree::{Diagnosis, Projected, absmean_row, measure, reference};
use trit_wspace::safetensors::Weights;
use trit_wspace::{Layer, Net, Trit};

fn identity(dim: usize) -> Vec<Vec<f64>> {
    (0..dim)
        .map(|i| {
            let mut v = vec![0.0; dim];
            v[i] = 1.0;
            v
        })
        .collect()
}

fn weights(rows: usize, cols: usize, values: Vec<f32>) -> Weights {
    Weights { rows, cols, values }
}

fn trit_i8(values: &[f32]) -> Vec<i8> {
    let w = weights(1, values.len(), values.to_vec());
    let layer = w.to_layer().expect("ternarize");
    (0..values.len())
        .map(|c| layer.trit(0, c).expect("trit").to_i8())
        .collect()
}

#[test]
fn identity_projection_matches_reference() {
    let q = weights(2, 3, vec![1.0, -2.0, 0.5, 0.0, 1.0, -1.0]);
    let readout = vec![1.0, -0.5];
    let basis = identity(3);
    let p = Projected::new(&q, &readout, &basis);
    for x in [
        [1.0f32, 0.0, -1.0],
        [0.5, 0.5, 0.5],
        [-1.0, 2.0, 0.0],
        [0.0, 0.0, 0.0],
    ] {
        assert_eq!(
            p.judge(p.weights(), &p.project_f32(&x)),
            reference(&q, &readout, &x),
            "x={x:?}"
        );
    }
}

#[test]
fn absmean_handles_zero_and_half() {
    assert_eq!(absmean_row(&[0.0, -0.0, 0.0]), vec![0.0, 0.0, 0.0]);
    assert_eq!(absmean_row(&[0.5, 0.5]), vec![1.0, 1.0]);
    assert_eq!(absmean_row(&[1.5, -1.5]), vec![1.0, -1.0]);
}

#[test]
fn full_forward_matches_net() {
    let q = weights(2, 3, vec![1.0, -1.0, 0.0, 0.0, 1.0, -1.0]);
    let readout = vec![1.0, -1.0];
    let basis = identity(3);
    let diag = Diagnosis::new(&q, &readout, &basis);
    let net = Net::new(vec![
        q.to_layer().expect("query"),
        Layer::new(1, 2, &trit_i8(&readout), &[0]).expect("readout"),
    ])
    .expect("net");
    for a in [-1i8, 0, 1] {
        for b in [-1i8, 0, 1] {
            for c in [-1i8, 0, 1] {
                let input = vec![
                    Trit::from_i8(a).expect("trit"),
                    Trit::from_i8(b).expect("trit"),
                    Trit::from_i8(c).expect("trit"),
                ];
                assert_eq!(
                    diag.full_forward(&input),
                    net.forward(&input).expect("f")[0],
                    "input={a},{b},{c}"
                );
            }
        }
    }
}

#[test]
fn measure_counts_trits_and_orders_encodings() {
    let w = weights(2, 3, vec![1.0, -1.0, 0.0, 0.0, 1.0, -1.0]);
    let net = Net::new(vec![
        w.to_layer().expect("query"),
        Layer::new(1, 2, &[1, -1], &[0]).expect("readout"),
    ])
    .expect("net");
    let size = measure(&net).expect("measure");
    // 权重 2×3 + 1×2，bias 2 + 1
    assert_eq!(size.trits, 6 + 2 + 3);
    assert!(size.wsp2 < size.wsp1, "2-bit 容器应小于文本");
    assert!(size.wsp2 > 0);
}
