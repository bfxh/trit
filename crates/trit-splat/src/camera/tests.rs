use super::*;
use crate::math::Vec3;

const SAMPLE: &str = r#"{
        "camera_model": "OPENCV", "fl_x": 1120.0, "fl_y": 1120.0,
        "cx": 480.0, "cy": 270.0, "w": 960, "h": 540,
        "frames": [
            {"file_path": "images/frame_0000.png",
             "transform_matrix": [[1,0,0,0],[0,1,0,2],[0,0,1,5],[0,0,0,1]]},
            {"file_path": "images/frame_0001.png",
             "transform_matrix": [[1,0,0,1],[0,1,0,2],[0,0,1,5],[0,0,0,1]]}
        ]
    }"#;

#[test]
fn parses_frames_and_intrinsics() {
    let t = parse_transforms(SAMPLE).unwrap();
    assert_eq!(t.frames.len(), 2);
    assert_eq!(t.intrinsics.width, 960);
    assert!((t.intrinsics.fl_x - 1120.0).abs() < 1e-4);
    let tr = t.frames[0].c2w.translation();
    assert!((tr.z - 5.0).abs() < 1e-5, "平移应为 (0,2,5)，实际 {tr:?}");
}

#[test]
fn point_on_optical_axis_maps_to_principal_point() {
    let t = parse_transforms(SAMPLE).unwrap();
    // 相机在 (0,2,5) 水平看 −Z：正前方 5m 的点 (0,2,0) 在光轴上 → 主点
    let (u, v, depth) = project_to_pixel(&t.intrinsics, &t.frames[0].c2w, Vec3::new(0.0, 2.0, 0.0));
    assert!((u - 480.0).abs() < 1e-3, "u={u} v={v} depth={depth}");
    assert!((v - 270.0).abs() < 1e-3, "u={u} v={v} depth={depth}");
    assert!((depth - 5.0).abs() < 1e-4, "u={u} v={v} depth={depth}");
}

#[test]
fn offset_point_displaces_right_and_up_correctly() {
    let t = parse_transforms(SAMPLE).unwrap();
    // 右 1m：(1,2,0) → u = 480 + 1120/5
    let (u, v, depth) = project_to_pixel(&t.intrinsics, &t.frames[0].c2w, Vec3::new(1.0, 2.0, 0.0));
    assert!((u - (480.0 + 1120.0 / 5.0)).abs() < 1e-3, "u={u}");
    assert!((v - 270.0).abs() < 1e-3, "v={v}");
    assert!((depth - 5.0).abs() < 1e-4);
    // 上 1m：(0,3,0) → +Y 朝上 ⇒ v 变小（图像向上）
    let (_u, v_up, _d) =
        project_to_pixel(&t.intrinsics, &t.frames[0].c2w, Vec3::new(0.0, 3.0, 0.0));
    assert!((v_up - (270.0 - 1120.0 / 5.0)).abs() < 1e-3, "v_up={v_up}");
}

#[test]
fn missing_frames_fails() {
    let err = parse_transforms(r#"{"w":960,"h":540}"#).unwrap_err();
    assert!(matches!(err, SplatError::Transforms(_)));
}
