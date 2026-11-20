//! 预览拼图（`contact_sheet`）的尺寸面：`--cols` 与清单里的 `cols`/`res`
//! 都是外部声明数，拼图宽高的乘积不能先分配后校验。

use trit_parts::splat::{MAX_SHEET_PIXELS, contact_sheet, sheet_dims};

fn frame(res: usize, v: f32) -> Vec<f32> {
    vec![v; res * res * 3]
}

#[test]
fn cols_that_overflow_the_width_are_rejected() {
    let frames = vec![frame(16, 0.5); 2];
    let err = contact_sheet(&frames, 16, usize::MAX).unwrap_err();
    // 只认「溢出」：`checked_mul` 被换成 `wrapping_mul` 时，绕出来的巨大值会被
    // 下一层的像素上限挡住并报「超过」——那说明溢出守卫已经没了，必须判红。
    assert!(err.contains("溢出"), "应报溢出，实得: {err}");
}

#[test]
fn cols_above_the_pixel_budget_are_rejected_before_allocating() {
    // 空帧列表 + 32 列 × 4096 边 = 131072×4096 ≈ 537M 像素 > 64M 上限。
    // 走 `sheet_dims`：只算尺寸、不分配，所以这一条真的不花内存。
    let err = sheet_dims(0, 4096, 32).unwrap_err();
    assert!(err.contains("超过"), "应报超过像素上限，实得: {err}");
}

#[test]
fn zero_res_is_rejected_instead_of_building_an_empty_sheet() {
    let frames = vec![frame(16, 0.5); 1];
    let err = contact_sheet(&frames, 0, 4).unwrap_err();
    assert!(err.contains("res") || err.contains("边"), "实得: {err}");
}

#[test]
fn legal_sheets_are_pinned() {
    let frames = vec![frame(4, 0.5), frame(4, 0.25)];
    let (w, h, sheet) = contact_sheet(&frames, 4, 2).expect("4×4 两帧两列必须合法");
    assert_eq!((w, h), (8, 4));
    assert_eq!(sheet.len(), 8 * 4 * 3);
}

#[test]
fn empty_frames_still_make_one_row() {
    let (w, h, sheet) = contact_sheet(&[], 256, 8).expect("空帧列表也要给一个 2048×256 的空表");
    assert_eq!((w, h), (2048, 256));
    assert_eq!(sheet.len(), 2048 * 256 * 3);
}

#[test]
fn cap_is_the_documented_pixel_budget() {
    assert_eq!(MAX_SHEET_PIXELS, 64 << 20);
    // 上限内（2048×256 = 512K 像素）必须放行，且结果空表。
    let (w, h, sheet) = contact_sheet(&[], 256, 8).unwrap();
    assert_eq!((w, h), (2048, 256));
    assert!(sheet.iter().all(|v| *v == 0.0), "空帧时整张表应为 0");
}
