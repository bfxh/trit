//! GGUF 的 I2_S（`TYPE_I2_S = 36`）读写分支：与 `TYPE_TRIT` 共存、边界与负例。
//!
//! `scales` 长度 1（且多行）= per-tensor（I2_S），长度 = 行数 = per-row（TYPE_TRIT）。

use trit_core::Ternary;
use trit_export::{Error, Model, TYPE_TRIT, Tensor, read, write};

fn trits(count: usize) -> Vec<Ternary> {
    (0..count)
        .map(|i| match i % 3 {
            0 => Ternary::False,
            1 => Ternary::Unknown,
            _ => Ternary::True,
        })
        .collect()
}

/// per-tensor scale 的 I2_S 张量（`scales` 只放 1 个）。
fn i2s_tensor(name: &str, rows: usize, cols: usize, scale: f32) -> Tensor {
    Tensor {
        name: name.into(),
        rows,
        cols,
        trits: trits(rows * cols),
        scales: vec![scale],
        bias: vec![Ternary::Unknown; rows],
    }
}

fn per_row_tensor() -> Tensor {
    Tensor {
        name: "encoder.weight".into(),
        rows: 3,
        cols: 7,
        trits: trits(21),
        scales: vec![0.5, 1.0, 1.5],
        bias: vec![Ternary::True, Ternary::False, Ternary::Unknown],
    }
}

fn model(tensors: Vec<Tensor>) -> Model {
    Model {
        metadata: Default::default(),
        tensors,
    }
}

/// GGUF 头里张量的类型码（读第一个张量的 info；只用于断言写出的类型）。
fn first_kind(bytes: &[u8]) -> u32 {
    let model = read(bytes).unwrap();
    assert_eq!(model.tensors.len(), 1);
    // 用读回结果推断：scales 长度 1 = I2_S
    if model.tensors[0].scales.len() == 1 {
        trit_export::TYPE_I2_S
    } else {
        TYPE_TRIT
    }
}

#[test]
fn i2s_round_trips_with_per_tensor_scale() {
    let m = model(vec![i2s_tensor("blk.0.ffn_down.weight", 4, 128, 1.5)]);
    let bytes = write(&m).unwrap();
    assert_eq!(first_kind(&bytes), trit_export::TYPE_I2_S);
    let back = read(&bytes).unwrap();
    let t = &back.tensors[0];
    assert_eq!(t.scales, vec![1.5]);
    assert_eq!(t.trits, m.tensors[0].trits);
    assert!(t.bias.iter().all(|b| *b == Ternary::Unknown));
}

#[test]
fn mixed_types_survive_round_trip() {
    let m = model(vec![
        per_row_tensor(),
        i2s_tensor("blk.0.ffn_down.weight", 2, 128, 2.0),
    ]);
    let bytes = write(&m).unwrap();
    let back = read(&bytes).unwrap();
    assert_eq!(back.tensors[0].scales, vec![0.5, 1.0, 1.5]);
    assert_eq!(back.tensors[1].scales, vec![2.0]);
    assert_eq!(back.tensors[1].trits, m.tensors[1].trits);
    assert_eq!(
        first_kind(&write(&model(vec![per_row_tensor()])).unwrap()),
        TYPE_TRIT
    );
}

#[test]
fn i2s_body_is_quarter_size_plus_tail() {
    // 512 个三值：I2_S 体 = 512/4 + 32 = 160（对齐后 160）；TYPE_TRIT 体 = ceil(512/5)
    // + 4×4 + 2 = 121（对齐后 128）。两者头/info 相同，文件大小差 32。
    let i2s = write(&model(vec![i2s_tensor("w", 4, 128, 1.0)]))
        .unwrap()
        .len();
    let per_row = write(&model(vec![Tensor {
        scales: vec![1.0; 4],
        ..i2s_tensor("w", 4, 128, 1.0)
    }]))
    .unwrap()
    .len();
    assert_eq!(i2s - per_row, 32, "I2_S({i2s}) vs TYPE_TRIT({per_row})");
}

#[test]
fn i2s_rejects_unaligned_counts_and_partial_scales() {
    // 21 个元素不是 128 的倍数：2-bit 块对不齐
    let bad = model(vec![i2s_tensor("w", 3, 7, 1.0)]);
    assert!(matches!(write(&bad), Err(Error::Tensor(_))));

    // 多行张量只允许 1 个（per-tensor）或 rows 个（per-row）scale
    let partial = model(vec![Tensor {
        scales: vec![1.0, 2.0],
        ..i2s_tensor("w", 3, 7, 1.0)
    }]);
    assert!(matches!(write(&partial), Err(Error::Field(_))));
}
