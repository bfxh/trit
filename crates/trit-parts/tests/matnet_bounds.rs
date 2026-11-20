//! `material-cnn --hidden` 是 `MatNet` 的隐藏宽度，直接乘进 `cout*cin*9`——
//! 平方级分配，必须先判上限。

use trit_parts::hidden::MAX_HIDDEN;
use trit_parts::matnet::MatNet;

#[test]
fn hidden_above_the_cap_is_rejected() {
    assert!(MatNet::new(MAX_HIDDEN, 7).is_ok(), "上限本身必须合法");
    let err = MatNet::new(MAX_HIDDEN + 1, 7).err().expect("越界必须 Err");
    assert!(
        err.contains("hidden") || err.contains("超过"),
        "应报 hidden 越界，实得: {err}"
    );
}

#[test]
fn hidden_of_usize_max_is_rejected_without_panicking() {
    // 旧路径：`cout*cin*9` 在 usize 里绕回成小值，随后 `vec![0.0; cout]` 直接吃掉 4e18 字节。
    let err = MatNet::new(usize::MAX, 7)
        .err()
        .expect("usize::MAX 必须 Err");
    assert!(
        err.contains("hidden") || err.contains("超过"),
        "应报 hidden 越界，实得: {err}"
    );
}

#[test]
fn zero_hidden_is_rejected() {
    assert!(
        MatNet::new(0, 7).is_err(),
        "0 通道会让池化与线性头全空，前向时除零"
    );
}

#[test]
fn legal_hidden_builds_the_documented_shape() {
    let net = MatNet::new(8, 7).expect("--hidden 8 是默认值，必须合法");
    assert_eq!(net.c1.cin, 3);
    assert_eq!(net.c1.cout, 8);
    assert_eq!(net.c2.cin, 8);
    assert_eq!(net.c2.cout, 8);
    assert_eq!(net.head_w.len(), 8 * 4);
    assert_eq!(net.c2.w.len(), 8 * 8 * 9);
}

#[test]
fn cap_is_the_documented_value() {
    assert_eq!(MAX_HIDDEN, 256);
    // 上限本身的分配规模：256²×9 个 f32 = 2.3 MB，三份（w/mw/vw）= 7 MB。
    let net = MatNet::new(MAX_HIDDEN, 3).unwrap();
    assert_eq!(net.c2.w.len(), MAX_HIDDEN * MAX_HIDDEN * 9);
}
