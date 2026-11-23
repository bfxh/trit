//! GGUF 导入数据区全局重叠校验回归（#625）。
//!
//! 修前：`tensor_data` 只查起点不越界，导入路径不比较任意两区间，共享 offset 时第二层
//! 静默读到第一层字节。修后：全部 `TensorInfo` 先做一次全局区间校验——`[offset, end)`
//! 不越出数据段，按 offset 排序后相邻区间不得相交，相交错误点名两张量。

use trit_texture::gguf::{GGUF_MAGIC, GgufError, ImportOptions, import_gguf};

const TYPE_F32: u32 = 0;

struct Fixture {
    name: String,
    dims: Vec<u64>,
    values: Vec<f32>,
    offset: u64,
}

/// 手工拼 GGUF v3：信息段后对齐到 32 字节，数据区按给定 offset 落位，尾部可追加尾随字节。
fn build(fixtures: &[Fixture], trailing: usize) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&GGUF_MAGIC.to_le_bytes());
    out.extend_from_slice(&3u32.to_le_bytes());
    out.extend_from_slice(&(fixtures.len() as u64).to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes());
    for fx in fixtures {
        out.extend_from_slice(&(fx.name.len() as u64).to_le_bytes());
        out.extend_from_slice(fx.name.as_bytes());
        out.extend_from_slice(&(fx.dims.len() as u32).to_le_bytes());
        for dim in &fx.dims {
            out.extend_from_slice(&dim.to_le_bytes());
        }
        out.extend_from_slice(&TYPE_F32.to_le_bytes());
        out.extend_from_slice(&fx.offset.to_le_bytes());
    }
    while out.len() % 32 != 0 {
        out.push(0);
    }
    let data_start = out.len();
    let max_end = fixtures
        .iter()
        .map(|fx| fx.offset as usize + fx.values.len() * 4)
        .max()
        .unwrap_or(0);
    out.resize(data_start + max_end, 0);
    for fx in fixtures {
        let at = data_start + fx.offset as usize;
        for (index, value) in fx.values.iter().enumerate() {
            out[at + index * 4..at + index * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
    }
    out.extend(std::iter::repeat_n(0u8, trailing));
    out
}

#[test]
fn rejects_overlapping_tensor_regions() {
    let bytes = build(
        &[
            Fixture {
                name: "a.weight".to_string(),
                dims: vec![2, 1],
                values: vec![1.0, -1.0],
                offset: 0,
            },
            Fixture {
                name: "b.weight".to_string(),
                dims: vec![2, 1],
                values: vec![1.0, -1.0],
                offset: 0,
            },
        ],
        0,
    );
    let err = import_gguf(&bytes, &ImportOptions::default()).unwrap_err();
    match err {
        GgufError::Overlap { first, second } => {
            assert_eq!(first, "a.weight");
            assert_eq!(second, "b.weight");
        }
        other => panic!("期望 Overlap，实际 {other:?}"),
    }
}

#[test]
fn accepts_disjoint_regions_and_trailing_bytes() {
    // a.weight 3×2 = 6 元素 24 字节 @0；bias 长度 2 @24；b.weight 2×1 = 8 字节 @32。
    let bytes = build(
        &[
            Fixture {
                name: "blk.0.ffn.weight".to_string(),
                dims: vec![3, 2],
                values: vec![1.0, -1.0, 0.0, 0.0, 1.0, -1.0],
                offset: 0,
            },
            Fixture {
                name: "blk.0.ffn.bias".to_string(),
                dims: vec![2],
                values: vec![0.9, -0.9],
                offset: 24,
            },
            Fixture {
                name: "blk.1.out.weight".to_string(),
                dims: vec![2, 1],
                values: vec![1.0, -1.0],
                offset: 32,
            },
        ],
        7,
    );
    let model = import_gguf(&bytes, &ImportOptions::default())
        .expect("不相交区间 + 数据区尾随字节必须仍为 Ok");
    assert_eq!(model.layers.len(), 2);
    assert_eq!(model.layers[0].weights, vec![1, -1, 0, 0, 1, -1]);
    assert_eq!(model.layers[1].weights, vec![1, -1]);
}
