//! 读侧重名查重回归（#624）。
//!
//! 写侧 [`write`](trit_export::write) 早已用 `BTreeSet` 查重并返回 `Error::Duplicate`；
//! 本测试对齐读侧 [`read`](trit_export::read)：信息段出现重名张量必须同样报错。

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

/// 手工拼 GGUF v3：两个同名 `w` 张量，体区零填充（重名在信息段解析阶段就该被拒）。
fn duplicate_name_bytes() -> Vec<u8> {
    let rows = 1usize;
    let cols = 5usize;
    let body = tensor_size(rows, cols);
    let mut out = Vec::new();
    out.extend_from_slice(b"GGUF");
    put_u32(&mut out, 3);
    put_u64(&mut out, 2);
    put_u64(&mut out, 0);
    for offset in [0u64, body as u64] {
        put_str(&mut out, "w");
        put_u32(&mut out, 2);
        put_u64(&mut out, cols as u64);
        put_u64(&mut out, rows as u64);
        put_u32(&mut out, 100);
        put_u64(&mut out, offset);
    }
    while out.len() % 32 != 0 {
        out.push(0);
    }
    out.resize(out.len() + body * 2, 0);
    out
}

#[test]
fn read_rejects_duplicate_tensor_name() {
    let bytes = duplicate_name_bytes();
    assert!(matches!(
        read(&bytes),
        Err(Error::Duplicate(name)) if name == "w"
    ));
}

#[test]
fn read_accepts_distinct_names() {
    let model = Model {
        metadata: Default::default(),
        tensors: vec![
            Tensor {
                name: "a.weight".to_string(),
                rows: 1,
                cols: 5,
                trits: vec![Ternary::False; 5],
                scales: vec![0.5],
                bias: vec![Ternary::False],
            },
            Tensor {
                name: "a.bias".to_string(),
                rows: 1,
                cols: 3,
                trits: vec![Ternary::False; 3],
                scales: vec![0.25],
                bias: vec![Ternary::False],
            },
        ],
    };
    let bytes = write(&model).expect("write");
    let back = read(&bytes).expect("不同名两张量应通过读侧重查");
    assert_eq!(back.tensors.len(), 2);
}
