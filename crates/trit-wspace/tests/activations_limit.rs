//! 显式上限重载的边界（从 `activations.rs` 拆出，守住行数预算）。

use trit_wspace::activations::{MAGIC, read_matrix, read_matrix_with_limit};

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
fn explicit_limit_is_enforced() {
    // 默认护栏（MAX_ELEMENTS）之外，调用方可以用显式上限读更大的合法矩阵；
    // 但显式上限本身必须生效，不能变成"永不拒绝"。
    let path = temp_path("limit");
    let values = [1.0f32, -0.5, 0.25, 3.5];
    write_matrix(&path, 2, 2, &values);
    assert!(read_matrix(&path).is_ok(), "默认上限下 4 元素应放行");
    assert!(
        read_matrix_with_limit(&path, 3).is_err(),
        "超过显式上限必须拒绝"
    );
    assert!(
        read_matrix_with_limit(&path, 4).is_ok(),
        "等于显式上限必须放行"
    );
    assert_eq!(read_matrix_with_limit(&path, 4).expect("read").2, values);
    let _ = std::fs::remove_file(&path);
}
