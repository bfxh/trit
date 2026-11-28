use super::{cam_at, gaussian, intr_small, pure_dc};
use crate::raster::render;

#[test]
fn degenerate_giant_splat_does_not_wash_frame() {
    // 复现 scan-wasteland 病理：266m 针状 splat（opacity≈1）贴着相机，
    // 屏幕 σ 全屏覆盖 → 闸门应剔除它，不干净色盖掉小 splat。
    let giant = gaussian([0.0, 0.0, 4.0], pure_dc([1.0, 0.0, 0.0]), 266.0);
    let small = gaussian([0.0, 0.0, 0.0], pure_dc([0.0, 0.0, 1.0]), 0.2);
    let frame = render(
        &[giant, small],
        &cam_at(5.0),
        &intr_small(),
        [0.0, 0.0, 0.0],
        None,
    );
    let o = (12 * 32 + 16) * 3;
    assert!(
        frame.rgb[o + 2] > 100,
        "小蓝 splat 应可见: r={} g={} b={}",
        frame.rgb[o],
        frame.rgb[o + 1],
        frame.rgb[o + 2]
    );
    assert!(
        frame.rgb[o] < 100,
        "巨红 splat 应被闸门剔除: r={}",
        frame.rgb[o]
    );
}
