//! TPM1 部署包的外部尺寸：层数、权重长度、通道数都先验后分。
//! `n` / `nw` / `cout` 是 u32，直接进 `with_capacity` 或 `vec![0.0; cout]` 能把进程打爆。

use trit_parts::tpm::TpmModel;

fn write_tpm(tag: &str, tail: &[u8]) -> std::path::PathBuf {
    let mut b = b"TPM1\0\0\0\0".to_vec();
    b.extend_from_slice(&1u32.to_le_bytes());
    b.extend_from_slice(tail);
    let p = std::env::temp_dir().join(format!("trit-tpm-bounds-{}-{tag}.tpm", std::process::id()));
    std::fs::write(&p, &b).unwrap();
    p
}

#[test]
fn absurd_layer_count_is_rejected_before_allocating() {
    // 10000 层 vs 0 字节剩余：with_capacity 只占 ~1 MB，但按文件读不出第一层。
    let p = write_tpm("layers", &10000u32.to_le_bytes());
    let err = TpmModel::load(&p).err().expect("层数越界必须 Err");
    assert!(err.contains("层数"), "实得: {err}");
    std::fs::remove_file(&p).ok();
}

#[test]
fn absurd_channel_count_is_rejected_before_allocating() {
    // n=1 层；层头 6 个 u32：cin/cout/kk/relu/convt/nw。cout=1000 时四份 f32 共 16 KB，
    // 但文件里没有数据，必须在分配前按剩余字节拒绝。
    let mut tail = 1u32.to_le_bytes().to_vec();
    for v in [1u32, 1000, 1, 1, 0, 1] {
        tail.extend_from_slice(&v.to_le_bytes());
    }
    tail.push(0);
    let p = write_tpm("channels", &tail);
    let err = TpmModel::load(&p).err().expect("通道越界必须 Err");
    assert!(err.contains("f32"), "应在分配 f32 块前拒绝，实得: {err}");
    std::fs::remove_file(&p).ok();
}

#[test]
fn truncated_weight_block_is_rejected() {
    let mut tail = 1u32.to_le_bytes().to_vec();
    for v in [1u32, 1, 1, 1, 0, u32::MAX] {
        tail.extend_from_slice(&v.to_le_bytes());
    }
    let p = write_tpm("weights", &tail);
    let err = TpmModel::load(&p).err().expect("长度越界必须 Err");
    assert!(err.contains("截断"), "实得: {err}");
    std::fs::remove_file(&p).ok();
}
