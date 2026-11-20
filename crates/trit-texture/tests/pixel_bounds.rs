//! PNG/容器的像素数上限：`check_pixels` 在 `vec![0u8; buffer_size]` 之前拒绝。
//!
//! `MAX_TEXTURE_PIXELS = 1<<22`，等价于 2048×2048；IHDR 的 u32 宽高直接进
//! `output_buffer_size()` 是 GB 级。

use trit_texture::MAX_TEXTURE_PIXELS;
use trit_texture::convert::check_pixels;

#[test]
fn the_cap_itself_is_legal() {
    assert_eq!(check_pixels(2048, 2048).unwrap(), MAX_TEXTURE_PIXELS);
    assert!(check_pixels(1, 1).is_ok());
}

#[test]
fn one_pixel_above_the_cap_is_rejected() {
    let err = check_pixels(MAX_TEXTURE_PIXELS + 1, 1)
        .err()
        .expect("越界必须 Err");
    assert!(err.to_string().contains("超过上限"), "实得: {err}");
}

#[test]
fn overflow_is_rejected_before_multiplying() {
    assert!(check_pixels(usize::MAX, 2).is_err());
    assert!(check_pixels(0, 1).is_ok(), "0 像素是空图，交给上层判");
}
