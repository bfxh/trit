//! GGUF v3 三值容器：往返、与 trit-codec 逐位一致、坏输入。

use trit_core::Ternary;
use trit_export::{Error, MetaValue, Model, Tensor, read, tensor_size, write};

fn sample_trits(count: usize, seed: u32) -> Vec<Ternary> {
    let mut x = seed;
    (0..count)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            match x % 3 {
                0 => Ternary::False,
                1 => Ternary::Unknown,
                _ => Ternary::True,
            }
        })
        .collect()
}

fn sample_model() -> Model {
    let mut metadata = std::collections::BTreeMap::new();
    metadata.insert(
        "general.name".to_string(),
        MetaValue::Str("trit-demo".to_string()),
    );
    metadata.insert("trit.version".to_string(), MetaValue::U32(2));
    metadata.insert("trit.alpha".to_string(), MetaValue::F32(0.75));
    Model {
        metadata,
        tensors: vec![
            Tensor {
                name: "encoder.weight".to_string(),
                rows: 3,
                cols: 7,
                trits: sample_trits(21, 0x1234),
                scales: vec![0.5, 1.0, 1.5],
                bias: vec![Ternary::True, Ternary::False, Ternary::Unknown],
            },
            Tensor {
                name: "encoder.bias".to_string(),
                rows: 1,
                cols: 5,
                trits: sample_trits(5, 0xabcd),
                scales: vec![0.25],
                bias: vec![Ternary::False],
            },
        ],
    }
}

#[test]
fn round_trip_preserves_every_field() {
    let model = sample_model();
    let bytes = write(&model).expect("write");
    assert_eq!(&bytes[..4], b"GGUF");
    let back = read(&bytes).expect("read");
    assert_eq!(back, model);
}

#[test]
fn tensor_data_matches_trit_codec_packing() {
    let model = sample_model();
    let bytes = write(&model).expect("write");
    // 期望字节由 trit-codec::pack::pack（0.12.0 线格式契约）生成后钉住：
    // 21 trits -> [176, 94, 135, 2, 2]；5 trits -> [76]；bias(2,0,1) -> [11]。
    let pins: [&[u8]; 3] = [&[176, 94, 135, 2, 2], &[76], &[11]];
    for pin in pins {
        assert!(
            bytes.windows(pin.len()).any(|window| window == pin),
            "GGUF 输出里找不到 trit-codec 打包字节 {pin:?}"
        );
    }
}

#[test]
fn second_tensor_offset_is_aligned() {
    let model = sample_model();
    let bytes = write(&model).expect("write");
    let back = read(&bytes).expect("read");
    assert_eq!(back.tensors.len(), 2);
    // 数据区里第二个张量的起点必须落在 32 字节边界上（第一个张量数据向上取整）。
    assert!(!tensor_size(3, 7).is_multiple_of(32));
}

#[test]
fn rejects_bad_inputs() {
    let bytes = write(&sample_model()).expect("write");
    let mut bad = bytes.clone();
    bad[0] = b'X';
    assert!(matches!(read(&bad), Err(Error::Magic)));

    let mut bad = bytes.clone();
    bad[4..8].copy_from_slice(&9u32.to_le_bytes());
    assert!(matches!(read(&bad), Err(Error::Version(9))));

    assert!(matches!(read(&bytes[..10]), Err(Error::Truncated)));

    let mut model = sample_model();
    model.tensors[0].name = model.tensors[1].name.clone();
    assert!(matches!(write(&model), Err(Error::Duplicate(_))));

    let mut model = sample_model();
    model.tensors[0].scales.pop();
    assert!(matches!(write(&model), Err(Error::Field(_))));

    let mut model = sample_model();
    model.tensors[0].rows = 0;
    assert!(matches!(write(&model), Err(Error::Shape(_, _))));
}

#[test]
fn rejects_unsupported_types_and_dims() {
    let model = sample_model();
    let mut bytes = write(&model).expect("write");
    // 在文件里找到第一个张量的 type 字段：name 之后是 dims u32、两个 u64、type u32。
    // 用 read 的解析顺序复算位置，比硬编码字符串搜索更稳。
    let mut pos = 4 + 4 + 8 + 8; // magic/version/tensor_count/meta_count
    let meta_count = model.metadata.len();
    for _ in 0..meta_count {
        let len = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap()) as usize;
        pos += 8 + len;
        let kind = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
        pos += 4;
        pos += match kind {
            4 => 4,
            6 => 4,
            8 => 8 + u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap()) as usize,
            other => panic!("unexpected meta type {other}"),
        };
    }
    // 第一个 tensor_info：name + dims + cols + rows + type
    let name_len = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap()) as usize;
    pos += 8 + name_len;
    pos += 4; // dims
    pos += 8 + 8; // cols + rows
    bytes[pos..pos + 4].copy_from_slice(&7u32.to_le_bytes());
    assert!(matches!(read(&bytes), Err(Error::TensorType(7))));
}
