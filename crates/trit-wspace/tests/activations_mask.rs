//! `WSPMSK1` 掩码格式的读取边界。

fn temp_path(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "trit-wspace-mask-{tag}-{}-{:?}.u8",
        std::process::id(),
        std::thread::current().id()
    ))
}

#[test]
fn reads_mask_round_trip_and_rejects_bad_input() {
    let path = temp_path("mask");
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"WSPMSK1\0");
    bytes.extend_from_slice(&2u32.to_le_bytes());
    bytes.extend_from_slice(&3u32.to_le_bytes());
    bytes.extend_from_slice(&[1, 1, 0, 1, 0, 0]);
    std::fs::write(&path, bytes).expect("write");
    let (batch, seq, mask) = trit_wspace::activations::read_mask(&path).expect("read");
    assert_eq!((batch, seq), (2, 3));
    assert_eq!(mask, vec![1, 1, 0, 1, 0, 0]);
    std::fs::write(&path, b"WSPMSK1\0\0\0\0\0\0\0\0\0\0\0\0").expect("write");
    assert!(trit_wspace::activations::read_mask(&path).is_err());
    let _ = std::fs::remove_file(&path);
}
