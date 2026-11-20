//! `tconv::Conv::forward` 的输入契约：`x.len()` 必须等于 `cin`，每个通道必须正好 `w×h`。
//! 少一个通道或短一截，`x[ci][yy * w + xr]` 就会越界。

use trit_parts::tconv::Conv;

#[test]
#[should_panic(expected = "输入必须 cin 个 w×h 通道")]
fn short_channel_is_rejected() {
    let c = Conv::new(1, 2, 1);
    let _ = c.forward(&[vec![0.0f32; 15]], 4, 4);
}

#[test]
#[should_panic(expected = "输入必须 cin 个 w×h 通道")]
fn wrong_channel_count_is_rejected() {
    let c = Conv::new(2, 2, 1);
    let _ = c.forward(&[vec![0.0f32; 16]], 4, 4);
}
