//! `WSPACT1` 激活矩阵格式的读取边界。

use trit_wspace::activations::{MAGIC, read_matrix};

fn temp_path(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "trit-wspace-act-{tag}-{}-{:?}.f32",
        std::process::id(),
        std::thread::current().id()
    ))
}

fn write_matrix(path: &std::path::Path, rows: u32, cols: u32, values: &[f32]) {
    let mut bytes = Vec::with_capacity(16 + values.len() * 4);
    bytes.extend_from_slice(&MAGIC);
    bytes.extend_from_slice(&rows.to_le_bytes());
    bytes.extend_from_slice(&cols.to_le_bytes());
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    std::fs::write(path, bytes).expect("write");
}

#[test]
fn reads_back_exact_values() {
    let path = temp_path("ok");
    let values = [1.0f32, -0.5, 0.25, 3.5];
    write_matrix(&path, 2, 2, &values);
    let (rows, cols, got) = read_matrix(&path).expect("read");
    assert_eq!((rows, cols), (2, 2));
    assert_eq!(got, values);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn rejects_bad_magic_short_and_length_mismatch() {
    let path = temp_path("bad");
    std::fs::write(&path, b"NOTACT1\0\0\0\0\0\0\0\0\0\0\0\0").expect("write");
    assert!(read_matrix(&path).is_err(), "坏魔数必须拒绝");

    std::fs::write(&path, b"WSPACT1\0").expect("write");
    assert!(read_matrix(&path).is_err(), "截断头必须拒绝");

    write_matrix(&path, 1, 2, &[1.0]);
    assert!(read_matrix(&path).is_err(), "数据长度不符必须拒绝");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn rejects_oversized_shape_before_allocating() {
    let path = temp_path("huge");
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&MAGIC);
    bytes.extend_from_slice(&u32::MAX.to_le_bytes());
    bytes.extend_from_slice(&u32::MAX.to_le_bytes());
    std::fs::write(&path, bytes).expect("write");
    let error = read_matrix(&path).expect_err("必须拒绝");
    assert!(error.contains("溢出") || error.contains("过大"));
    let _ = std::fs::remove_file(&path);
}
