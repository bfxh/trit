//! GGUF 读取预算边界（拆出主测试文件守模块行数）。

use trit_export::{Error, read};

#[test]
fn rejects_huge_tensor_count_before_allocation() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"GGUF");
    bytes.extend_from_slice(&3u32.to_le_bytes());
    bytes.extend_from_slice(&100_001u64.to_le_bytes()); // tensor_count：刚超过 100k 上限
    bytes.extend_from_slice(&0u64.to_le_bytes()); // meta_count
    assert!(matches!(read(&bytes), Err(Error::TooMany { .. })));
}
