//! 端到端小场景：无高斯区域应保留纯背景色。

mod common;
use common::{TRANSFORMS, build_ply};

use trit_splat::camera::parse_transforms;
use trit_splat::ply::parse_ply;
use trit_splat::raster::render;

#[test]
fn background_where_no_splats() {
    let gaussians = parse_ply(&build_ply()).unwrap();
    let t = parse_transforms(TRANSFORMS).unwrap();
    let frame = render(
        &gaussians,
        &t.frames[0].c2w,
        &t.intrinsics,
        [1.0, 1.0, 1.0],
        None,
    );
    // 左上角远离三颗高斯 → 纯背景
    assert_eq!(frame.rgb[0], 255);
}
