//! 层宽上限：`--hidden` 与 JSON 两条入口都必须在分配前拒绝。
//! `MAX_WIDTH=4096` 时 `4096²` 权重已经是 16.8M 个 i8（训练态还有 f32 影子）。

use trit_net::{MAX_WIDTH, Model};

#[test]
fn layer_width_above_the_cap_is_rejected() {
    let json = format!(
        r#"{{"layers":[{{"inputs":5,"outputs":{},"weights":[],"bias":[]}}]}}"#,
        MAX_WIDTH + 1
    );
    let err = Model::from_json(&json).err().expect("越界必须 Err");
    assert!(err.to_string().contains("层宽"), "实得: {err}");
}

#[test]
fn a_normal_layer_is_accepted() {
    let json = r#"{"layers":[{"inputs":1,"outputs":1,"weights":[[1]],"bias":[1]}]}"#;
    let model = Model::from_json(json).expect("合法模型");
    assert_eq!(model.layers[0].outputs, 1);
}
