//! `Conv::new` 的尺寸自守：TPC 加载、`--conv`、三个模型文本加载器共用这一个构造。
//!
//! `cout·cin·9` 会被 `w`/`mw`/`vw` 各分配一份；2048² 已经是 151 MB/份。
//! 校验必须在任何分配之前，且 `usize` 上先乘后判会被绕回骗过。

use trit_parts::depthnet::DepthNet;
use trit_parts::depthunet::DepthUNet;
use trit_parts::hidden::MAX_HIDDEN;
use trit_parts::tconv::{Conv, MAX_CH, Net, check_ch};

#[test]
fn channel_above_the_cap_is_rejected() {
    assert!(Conv::new(MAX_CH, 1, 1).is_ok(), "上限本身必须合法");
    let err = Conv::new(MAX_CH + 1, 1, 1).err().expect("越界必须 Err");
    assert!(err.contains("通道"), "应报越界，实得: {err}");
}

#[test]
fn zero_channel_is_rejected() {
    assert!(Conv::new(0, 1, 1).is_err(), "0 输入通道没有意义");
    assert!(Conv::new(1, 0, 1).is_err(), "0 输出通道没有意义");
}

#[test]
fn huge_channel_is_rejected_before_multiplying() {
    // 先判通道再乘：`usize::MAX × 2 × 9` 若先乘，debug 下 panic、release 下绕成小数。
    let err = Conv::new(usize::MAX, 2, 1).err().expect("越界必须 Err");
    assert!(err.contains("通道"), "越界必须先于乘法，实得: {err}");
}

#[test]
fn weight_count_matches_the_layout() {
    assert_eq!(check_ch(3, 4).unwrap(), 3 * 4 * 9);
    assert!(check_ch(MAX_CH, MAX_CH).is_ok(), "上限之积仍在 usize 内");
}

#[test]
fn net_new_forwards_the_channel_check() {
    assert!(Net::new(1, &[4], 1).is_ok());
    let err = Net::new(1, &[MAX_CH + 1], 1).err().expect("越界必须 Err");
    assert!(err.contains("通道"), "实得: {err}");
}

#[test]
fn depth_net_rejects_hidden_above_the_cap() {
    assert!(DepthNet::new(MAX_HIDDEN, 1).is_ok(), "上限本身必须合法");
    let err = DepthNet::new(MAX_HIDDEN + 1, 1)
        .err()
        .expect("越界必须 Err");
    assert!(err.contains("隐藏层宽度"), "实得: {err}");
}

#[test]
fn depth_unet_rejects_hidden_above_the_cap() {
    assert!(DepthUNet::new(3, MAX_HIDDEN, 1, true).is_ok());
    let err = DepthUNet::new(3, MAX_HIDDEN + 1, 1, true)
        .err()
        .expect("越界必须 Err");
    assert!(err.contains("隐藏层宽度"), "实得: {err}");
}
