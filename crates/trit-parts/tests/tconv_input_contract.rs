//! `tconv::Net::input` 的输入长度契约：`img` 必须正好 `w×h`，短了不许进网络。

use trit_parts::tconv::Net;

#[test]
fn short_img_is_rejected_before_forward() {
    let net = Net::new(1, &[4], 7);
    let (w, h) = (4, 3);
    let short = vec![0.0f32; w * h - 1];
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        net.forward(&short, None, w, h)
    }));
    assert!(r.is_err(), "img 短一个像素就必须在分配/前向前被拒");
}

#[test]
fn exact_size_img_still_works() {
    let net = Net::new(1, &[4], 7);
    let (w, h) = (4, 3);
    let ok = vec![0.0f32; w * h];
    let out = net.forward(&ok, None, w, h);
    assert_eq!(out.len(), w * h);
}
