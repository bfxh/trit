//! trit-net `Model` 与 TTX MOD 容器的兼容对拍：weights/bias 无损往返，
//! 层间衔接语义一致（容器拒绝 trit-net 也拒绝的断裂链）。

use trit_net::{Layer, Model};
use trit_texture::ttx2::{TritModel, TritModelLayer, TtxDocument};

fn trit_net_model() -> Model {
    // 4 → 3 → 2 的合法链，权重/偏置覆盖 {-1,0,+1}。
    Model {
        layers: vec![
            Layer {
                inputs: 4,
                outputs: 3,
                weights: vec![vec![1, -1, 0, 1], vec![0, 0, -1, 0], vec![-1, 1, 1, 0]],
                bias: vec![0, 1, -1],
            },
            Layer {
                inputs: 3,
                outputs: 2,
                weights: vec![vec![1, 0, -1], vec![-1, -1, 1]],
                bias: vec![1, 0],
            },
        ],
    }
}

#[test]
fn trit_net_model_roundtrips_through_ttx_losslessly() {
    let model = trit_net_model();
    model.validate().unwrap();
    let layers: Vec<TritModelLayer> = model
        .layers
        .iter()
        .map(|layer| TritModelLayer::new(&layer.weights, &layer.bias).expect("合法层"))
        .collect();
    let document = TtxDocument::Model(TritModel::new(layers).unwrap());
    let container = trit_texture::ttx2::to_ttx2(&document).unwrap();

    let TtxDocument::Model(back) = trit_texture::ttx2::from_ttx2(&container).unwrap() else {
        panic!("kind 漂移");
    };
    // 无损判据：形状、权重、偏置逐项一致（rows() 给回 trit-net 的形状）。
    assert_eq!(back.layers.len(), model.layers.len());
    for (layer, original) in back.layers.iter().zip(&model.layers) {
        assert_eq!(
            (layer.inputs, layer.outputs),
            (original.inputs, original.outputs)
        );
        assert_eq!(layer.rows(), original.weights);
        assert_eq!(layer.bias, original.bias);
    }
}

#[test]
fn container_rejects_the_same_broken_chains_as_trit_net() {
    // 4 → 3 的层后接 inputs=2 的层：trit-net validate 与 TTX 容器都拒绝。
    let mut model = trit_net_model();
    model.layers[1].inputs = 2;
    model.layers[1].weights = vec![vec![1, 0], vec![0, -1]];
    assert!(model.validate().is_err());

    let layers: Vec<TritModelLayer> = model
        .layers
        .iter()
        .map(|layer| TritModelLayer::new(&layer.weights, &layer.bias).unwrap())
        .collect();
    assert!(
        TritModel::new(layers).is_err(),
        "容器必须与 trit-net 同语义拒绝"
    );
}
