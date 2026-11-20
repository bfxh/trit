//! GGUF 的 `tensor_count` 先验后分：`Vec::with_capacity(tensor_count)` 不按文件长度分配。
//!
//! `tensor_count=1000` 而文件只剩 0 字节：正常路径必须报 `TooMany`（含“张量数”），
//! 变异放行后会走到 `with_capacity(1000)` 再在读第一个 info 时报截断。

use trit_export::read;

#[test]
fn absurd_tensor_count_is_rejected_before_allocating() {
    let mut b = b"GGUF".to_vec();
    b.extend_from_slice(&3u32.to_le_bytes());
    b.extend_from_slice(&1000u64.to_le_bytes()); // tensor_count
    b.extend_from_slice(&0u64.to_le_bytes()); // meta_count
    let err = read(&b).err().expect("巨大 tensor_count 必须 Err");
    assert!(err.to_string().contains("张量数"), "实得: {err}");
}
