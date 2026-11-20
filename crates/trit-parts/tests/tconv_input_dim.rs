//! `tconv::VDen::input_dim` 必须与 `predict_x0` 真正构造的通道数一致：
//! `[x_t, t/T] + extra + cond`。少算 `extra` 会让调用方按错的 cin 建网。

use trit_parts::diffuse::Denoiser;
use trit_parts::tconv::{Net, VDen};

#[test]
fn vden_input_dim_counts_extra_channels() {
    let mut net = Net::new(4, &[], 3).unwrap(); // 2 基础 + 2 extra
    net.linear_out = true;
    let d = VDen {
        net,
        abar: vec![1.0, 0.0],
        extra: vec![0.5, 0.7],
        clamp: true,
        class: None,
    };
    assert_eq!(d.input_dim(), 4);
    // 通道数与 net.cin 一致，不会被 Conv::forward 的断言挡住
    let x0 = d.predict_x0(&[0.5f32; 16], 0.5, None, 4, 4);
    assert_eq!(x0.len(), 16);
}
