//! 端到端小场景：内存构造 PLY + transforms.json → render → 断言三色像素。

mod common;
use common::{TRANSFORMS, build_ply};

use trit_splat::camera::parse_transforms;
use trit_splat::ply::parse_ply;
use trit_splat::raster::render;

#[test]
fn three_splats_render_three_colors() {
    let gaussians = parse_ply(&build_ply()).unwrap();
    assert_eq!(gaussians.len(), 3);
    let t = parse_transforms(TRANSFORMS).unwrap();
    let frame = render(
        &gaussians,
        &t.frames[0].c2w,
        &t.intrinsics,
        [0.0, 0.0, 0.0],
        None,
    );
    assert_eq!(frame.rgb.len(), 32 * 24 * 3);

    let px = |x: usize, y: usize, c: usize| frame.rgb[(y * 32 + x) * 3 + c] as i32;
    // 上方偏红 / 中心偏绿 / 下方偏蓝（相机在 z=5 看 −Z：+Y 世界方向映射到屏幕上方）
    assert!(
        px(16, 6, 0) > px(16, 6, 1),
        "上方应偏红: r={} g={} b={} | 中 r={} g={} | 下 r={} g={} b={}",
        px(16, 6, 0),
        px(16, 6, 1),
        px(16, 6, 2),
        px(16, 12, 0),
        px(16, 12, 1),
        px(16, 18, 0),
        px(16, 18, 1),
        px(16, 18, 2),
    );
    assert!(px(16, 12, 1) > px(16, 12, 0), "中心应偏绿");
    assert!(px(16, 18, 2) > px(16, 18, 1), "下方应偏蓝");
}
