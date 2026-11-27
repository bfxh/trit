//! 读侧张量数据区全局不相交回归（R41-01）。
//!
//! [`read`](trit_export::read) 在信息段收集完成、数据段起点对齐后，必须对所有张量
//! `[start, end)` 做一次全局两两不相交复核：按起点排序后相邻区间相交即拒，首尾相接
//! `[a, b)` 与 `[b, c)` 允许。本文件覆盖：重叠拒绝、合法不重叠往返、相邻边界相接、
//! 零形状拒绝、张量区与信息段互不重叠的布局不变量。

use trit_core::Ternary;
use trit_export::{Error, Model, Tensor, read, tensor_size, write};

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_str(out: &mut Vec<u8>, text: &str) {
    put_u64(out, text.len() as u64);
    out.extend_from_slice(text.as_bytes());
}

struct Spec {
    name: &'static str,
    rows: usize,
    cols: usize,
    offset: u64,
}

/// 手工拼 GGUF v3：0 元数据，按声明信息段写头，对齐到 32 字节后补 `body_pad` 零字节。
fn build(specs: &[Spec], body_pad: usize) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"GGUF");
    put_u32(&mut out, 3);
    put_u64(&mut out, specs.len() as u64);
    put_u64(&mut out, 0);
    for spec in specs {
        put_str(&mut out, spec.name);
        put_u32(&mut out, 2);
        put_u64(&mut out, spec.cols as u64);
        put_u64(&mut out, spec.rows as u64);
        put_u32(&mut out, 100);
        put_u64(&mut out, spec.offset);
    }
    while out.len() % 32 != 0 {
        out.push(0);
    }
    out.resize(out.len() + body_pad, 0);
    out
}

fn sample_model() -> Model {
    Model {
        metadata: Default::default(),
        tensors: vec![
            Tensor {
                name: "a.weight".to_string(),
                rows: 1,
                cols: 5,
                trits: vec![
                    Ternary::False,
                    Ternary::True,
                    Ternary::Unknown,
                    Ternary::False,
                    Ternary::True,
                ],
                scales: vec![0.5],
                bias: vec![Ternary::False],
            },
            Tensor {
                name: "b.weight".to_string(),
                rows: 1,
                cols: 5,
                trits: vec![
                    Ternary::True,
                    Ternary::False,
                    Ternary::True,
                    Ternary::Unknown,
                    Ternary::False,
                ],
                scales: vec![2.5],
                bias: vec![Ternary::True],
            },
        ],
    }
}

/// (a) 两张量同 offset、数据区完全重叠：必须拒绝，错误点名两个张量。
#[test]
fn rejects_overlapping_tensor_regions() {
    let body = tensor_size(1, 5);
    let bytes = build(
        &[
            Spec {
                name: "a.weight",
                rows: 1,
                cols: 5,
                offset: 0,
            },
            Spec {
                name: "b.weight",
                rows: 1,
                cols: 5,
                offset: 0,
            },
        ],
        body * 2,
    );
    let error = read(&bytes).expect_err("重叠数据区必须被拒");
    match error {
        Error::Tensor(message) => {
            assert!(
                message.contains("a.weight"),
                "错误未点名 a.weight: {message}"
            );
            assert!(
                message.contains("b.weight"),
                "错误未点名 b.weight: {message}"
            );
            assert!(message.contains("重叠"), "错误未标注重叠: {message}");
        }
        other => panic!("预期 Tensor 区间错误，得到 {other:?}"),
    }
}

/// (b) 完全不重叠的合法模型往返后仍 `Ok`。
#[test]
fn accepts_disjoint_regions_round_trip() {
    let bytes = write(&sample_model()).expect("write");
    let back = read(&bytes).expect("合法不重叠模型必须通过");
    assert_eq!(back.tensors.len(), 2);
    assert_eq!(back, sample_model());
}

/// (c) 相邻边界 `A.end == B.start`：首尾相接允许，不算相交。
#[test]
fn accepts_adjacent_boundary_regions() {
    let body = tensor_size(1, 5);
    let bytes = build(
        &[
            Spec {
                name: "a.weight",
                rows: 1,
                cols: 5,
                offset: 0,
            },
            Spec {
                name: "b.weight",
                rows: 1,
                cols: 5,
                offset: body as u64,
            },
        ],
        body * 2,
    );
    let back = read(&bytes).expect("首尾相接区间必须通过");
    assert_eq!(back.tensors.len(), 2);
}

/// (d) 零行/列张量在信息段解析阶段即被 `Shape` 拒绝；不存在零长体区间。
///
/// 半开区间 `[s, s)` 与任何区间都不相交，因此即便后续出现零长张量，两两不相交
/// 判定也不会误拒；当前格式 `rows`/`cols` 为 0 先被既有形状闸门拦下。
#[test]
fn rejects_zero_shape_before_region_check() {
    let bytes = build(
        &[Spec {
            name: "a.weight",
            rows: 0,
            cols: 5,
            offset: 0,
        }],
        tensor_size(1, 5),
    );
    assert!(matches!(read(&bytes), Err(Error::Shape(0, 5))));
}

/// (d2) 两张最小 `1×1` 张量不重叠：各自 6 字节，首尾相接，必须通过。
#[test]
fn accepts_minimal_disjoint_regions() {
    let body = tensor_size(1, 1);
    let bytes = build(
        &[
            Spec {
                name: "a.weight",
                rows: 1,
                cols: 1,
                offset: 0,
            },
            Spec {
                name: "b.weight",
                rows: 1,
                cols: 1,
                offset: body as u64,
            },
        ],
        body * 2,
    );
    let back = read(&bytes).expect("最小不重叠区间必须通过");
    assert_eq!(back.tensors.len(), 2);
}

/// (e) 张量区不得伸入信息段：偏移相对对齐后的数据起点，非负偏移保证
/// `start >= data_start >= 信息段终点`；合法文件体区落在信息段之后。
#[test]
fn tensor_region_stays_after_info_section() {
    let body = tensor_size(1, 5);
    let bytes = build(
        &[Spec {
            name: "a.weight",
            rows: 1,
            cols: 5,
            offset: 0,
        }],
        body,
    );
    let back = read(&bytes).expect("单张量合法文件必须通过");
    assert_eq!(back.tensors.len(), 1);
}
