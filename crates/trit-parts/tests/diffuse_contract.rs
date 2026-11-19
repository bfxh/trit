//! `diffuse::PatchMlp::features` 的 xt 长度契约：`xt.len()` 必须等于 `w×h`，
//! 否则 `xt[yy * w + xx]` 会在 `x0.iter().zip(eps)` 静默截断后越界。

use trit_parts::diffuse::PatchMlp;

#[test]
#[should_panic(expected = "xt 长度必须等于 w×h")]
fn short_xt_is_rejected() {
    let d = PatchMlp::new(3, 0, 8, 1);
    let _ = d.features(&[0.0f32; 3], 0, 0, 0.5, None, 2, 2);
}
