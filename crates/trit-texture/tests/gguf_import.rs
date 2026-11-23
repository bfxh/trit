//! GGUF → MOD 导入既有回归（从 `src/gguf.rs` 内联测试拆出，控制行数预算）。
//!
//! 覆盖：链式层 + 偏置、阈值稀疏化、F16/BF16 解码、坏 magic/版本/量化类型、链不衔接/
//! 缺张量、非有限值与截断、TTX2 往返、NaN 阈值拒绝。

use trit_texture::gguf::{GGUF_MAGIC, GgufError, ImportOptions, import_gguf};
use trit_texture::ttx2::{TtxDocument, from_ttx2, to_ttx2};

const TYPE_F32: u32 = 0;
const TYPE_F16: u32 = 1;
const TYPE_BF16: u32 = 30;

struct Fixture {
    name: String,
    dims: Vec<u64>,
    type_id: u32,
    raw: Vec<u8>,
}

fn write_string(out: &mut Vec<u8>, text: &str) {
    out.extend_from_slice(&(text.len() as u64).to_le_bytes());
    out.extend_from_slice(text.as_bytes());
}

fn f32_tensor(name: &str, dims: &[u64], values: &[f32]) -> Fixture {
    Fixture {
        name: name.to_string(),
        dims: dims.to_vec(),
        type_id: TYPE_F32,
        raw: values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect(),
    }
}

fn raw_tensor(name: &str, dims: &[u64], type_id: u32, raw: Vec<u8>) -> Fixture {
    Fixture {
        name: name.to_string(),
        dims: dims.to_vec(),
        type_id,
        raw,
    }
}

fn build(tensors: &[Fixture]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&GGUF_MAGIC.to_le_bytes());
    out.extend_from_slice(&3u32.to_le_bytes());
    out.extend_from_slice(&(tensors.len() as u64).to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes());
    let mut offset = 0u64;
    for tensor in tensors {
        write_string(&mut out, &tensor.name);
        out.extend_from_slice(&(tensor.dims.len() as u32).to_le_bytes());
        for dim in &tensor.dims {
            out.extend_from_slice(&dim.to_le_bytes());
        }
        out.extend_from_slice(&tensor.type_id.to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += tensor.raw.len() as u64;
    }
    while out.len() % 32 != 0 {
        out.push(0);
    }
    for tensor in tensors {
        out.extend_from_slice(&tensor.raw);
    }
    out
}

fn two_layer_fixture() -> Vec<u8> {
    build(&[
        f32_tensor(
            "blk.0.ffn.weight",
            &[3, 2],
            &[1.0, -1.0, 0.0, 0.0, 1.0, -1.0],
        ),
        f32_tensor("blk.0.ffn.bias", &[2], &[0.9, -0.9]),
        f32_tensor("blk.1.out.weight", &[2, 1], &[1.0, -1.0]),
    ])
}

#[test]
fn imports_chained_layers_with_bias() {
    let model = import_gguf(&two_layer_fixture(), &ImportOptions::default()).unwrap();
    assert_eq!(model.layers.len(), 2);
    assert_eq!((model.layers[0].inputs, model.layers[0].outputs), (3, 2));
    assert_eq!(model.layers[0].weights, vec![1, -1, 0, 0, 1, -1]);
    assert_eq!(model.layers[0].bias, vec![1, -1]);
    assert_eq!((model.layers[1].inputs, model.layers[1].outputs), (2, 1));
    assert_eq!(model.layers[1].weights, vec![1, -1]);
    assert_eq!(model.layers[1].bias, vec![0], "缺偏置时补 0");
}

#[test]
fn zero_threshold_creates_sparsity() {
    let bytes = build(&[f32_tensor("a.weight", &[3, 1], &[0.2, -0.3, 0.8])]);
    let options = ImportOptions {
        zero_threshold: 0.25,
        ..ImportOptions::default()
    };
    let model = import_gguf(&bytes, &options).unwrap();
    assert_eq!(model.layers[0].weights, vec![0, -1, 1]);
}

#[test]
fn half_and_bfloat16_are_decoded() {
    let mut f16 = Vec::new();
    for bits in [0x3C00u16, 0xBC00, 0x3800] {
        f16.extend_from_slice(&bits.to_le_bytes());
    }
    let bytes = build(&[raw_tensor("a.weight", &[3, 1], TYPE_F16, f16)]);
    let model = import_gguf(&bytes, &ImportOptions::default()).unwrap();
    assert_eq!(model.layers[0].weights, vec![1, -1, 1]);

    let bf16: Vec<u8> = [0x3F80u16, 0xBF80]
        .iter()
        .flat_map(|bits| bits.to_le_bytes())
        .collect();
    let bytes = build(&[raw_tensor("a.weight", &[2, 1], TYPE_BF16, bf16)]);
    let model = import_gguf(&bytes, &ImportOptions::default()).unwrap();
    assert_eq!(model.layers[0].weights, vec![1, -1]);
}

#[test]
fn rejects_bad_magic_version_and_quantized_types() {
    let mut bytes = two_layer_fixture();
    bytes[0] ^= 0xFF;
    assert!(matches!(
        import_gguf(&bytes, &ImportOptions::default()),
        Err(GgufError::Magic(_))
    ));

    let mut bytes = two_layer_fixture();
    bytes[4..8].copy_from_slice(&1u32.to_le_bytes());
    assert!(matches!(
        import_gguf(&bytes, &ImportOptions::default()),
        Err(GgufError::Version(1))
    ));

    // Q2_K = 10：未验证的量化布局必须显式拒绝。
    let bytes = build(&[raw_tensor("a.weight", &[2, 1], 10, vec![0; 4])]);
    assert!(matches!(
        import_gguf(&bytes, &ImportOptions::default()),
        Err(GgufError::UnsupportedType { type_id: 10, .. })
    ));
}

#[test]
fn rejects_chain_mismatch_and_missing_tensor() {
    let bytes = build(&[
        f32_tensor("blk.0.w.weight", &[4, 2], &[1.0; 8]),
        f32_tensor("blk.1.w.weight", &[3, 1], &[1.0; 3]),
    ]);
    assert!(matches!(
        import_gguf(&bytes, &ImportOptions::default()),
        Err(GgufError::Chain(_))
    ));

    let bytes = two_layer_fixture();
    let options = ImportOptions {
        tensor_names: Some(vec!["nope".to_string()]),
        ..ImportOptions::default()
    };
    assert!(matches!(
        import_gguf(&bytes, &options),
        Err(GgufError::Missing(name)) if name == "nope"
    ));
}

#[test]
fn rejects_non_finite_and_truncated() {
    let bytes = build(&[f32_tensor("a.weight", &[2, 1], &[f32::NAN, 1.0])]);
    assert!(matches!(
        import_gguf(&bytes, &ImportOptions::default()),
        Err(GgufError::Value { .. })
    ));

    let mut bytes = two_layer_fixture();
    bytes.truncate(24);
    assert!(matches!(
        import_gguf(&bytes, &ImportOptions::default()),
        Err(GgufError::Truncated)
    ));

    // 数据段已对齐但少一个字节：必须由"元素长度"检查报错，不能靠切片 panic。
    let mut bytes = two_layer_fixture();
    let length = bytes.len();
    bytes.truncate(length - 1);
    assert!(matches!(
        import_gguf(&bytes, &ImportOptions::default()),
        Err(GgufError::Truncated)
    ));

    let bytes = two_layer_fixture();
    let options = ImportOptions {
        max_bytes: 8,
        ..ImportOptions::default()
    };
    assert!(matches!(
        import_gguf(&bytes, &options),
        Err(GgufError::TooLarge { .. })
    ));
}

#[test]
fn imported_model_round_trips_through_ttx2() {
    let model = import_gguf(&two_layer_fixture(), &ImportOptions::default()).unwrap();
    let container = to_ttx2(&TtxDocument::Model(model.clone())).expect("MOD 写出");
    match from_ttx2(&container).expect("MOD 读回") {
        TtxDocument::Model(back) => assert_eq!(back, model),
        other => panic!("kind 不对：{other:?}"),
    }
}

#[test]
fn unsupported_nan_threshold_is_rejected() {
    let bytes = two_layer_fixture();
    let options = ImportOptions {
        zero_threshold: f32::NAN,
        ..ImportOptions::default()
    };
    assert!(matches!(
        import_gguf(&bytes, &options),
        Err(GgufError::Metadata(_))
    ));
}
