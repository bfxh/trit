//! 端到端小场景：内存构造 PLY + transforms.json → render → 断言像素。

use trit_splat::camera::parse_transforms;
use trit_splat::ply::parse_ply;
use trit_splat::raster::render;

const TRANSFORMS: &str = r#"{
    "camera_model": "OPENCV", "fl_x": 50.0, "fl_y": 50.0,
    "cx": 16.0, "cy": 12.0, "w": 32, "h": 24,
    "frames": [{"file_path": "images/frame_0000.png",
                "transform_matrix": [[1,0,0,0],[0,1,0,0],[0,0,1,5],[0,0,0,1]]}]
}"#;

fn build_ply() -> Vec<u8> {
    // 三颗高斯：红在光轴上方，绿在光轴上，蓝在光轴下方
    // 属性顺序: x y z f_dc0..2 opacity scale0..2 rot0..3(xyzw)
    let head = "ply\nformat binary_little_endian 1.0\nelement vertex 3\n\
         property float x\nproperty float y\nproperty float z\n\
         property float f_dc_0\nproperty float f_dc_1\nproperty float f_dc_2\n\
         property float opacity\nproperty float scale_0\nproperty float scale_1\n\
         property float scale_2\nproperty float rot_0\nproperty float rot_1\n\
         property float rot_2\nproperty float rot_3\nend_header\n";
    let items: [([f32; 3], [f32; 3]); 3] = [
        // DC 系数按 color = f_dc·C0 + 0.5 配出纯色
        ([0.0, 1.0, 0.0], [1.772, -1.772, -1.772]),
        ([0.0, 0.0, 0.0], [-1.772, 1.772, -1.772]),
        ([0.0, -1.0, 0.0], [-1.772, -1.772, 1.772]),
    ];
    let mut out = head.as_bytes().to_vec();
    for (pos, dc) in items {
        let vals: [f32; 14] = [
            pos[0], pos[1], pos[2], dc[0], dc[1], dc[2],
            1e8, // opacity logit → sigmoid ≈ 1
            0.3f32.ln(), 0.3f32.ln(), 0.3f32.ln(),
            0.0, 0.0, 0.0, 1.0, // rot xyzw = 单位旋转
        ];
        for f in vals {
            out.extend_from_slice(&f.to_le_bytes());
        }
    }
    out
}

#[test]
fn three_splats_render_three_colors() {
    let gaussians = parse_ply(&build_ply()).unwrap();
    assert_eq!(gaussians.len(), 3);
    let t = parse_transforms(TRANSFORMS).unwrap();
    let frame = render(&gaussians, &t.frames[0].c2w, &t.intrinsics, [0.0, 0.0, 0.0], None);
    assert_eq!(frame.rgb.len(), 32 * 24 * 3);

    let px = |x: usize, y: usize, c: usize| frame.rgb[(y * 32 + x) * 3 + c] as i32;
    // 上方偏红 / 中心偏绿 / 下方偏蓝（相机在 z=5 看 −Z：+Y 世界方向映射到屏幕上方）
    assert!(
        px(16, 6, 0) > px(16, 6, 1),
        "上方应偏红: r={} g={} b={} | 中 r={} g={} | 下 r={} g={} b={}",
        px(16, 6, 0), px(16, 6, 1), px(16, 6, 2),
        px(16, 12, 0), px(16, 12, 1),
        px(16, 18, 0), px(16, 18, 1), px(16, 18, 2),
    );
    assert!(px(16, 12, 1) > px(16, 12, 0), "中心应偏绿");
    assert!(px(16, 18, 2) > px(16, 18, 1), "下方应偏蓝");
}

#[test]
fn background_where_no_splats() {
    let gaussians = parse_ply(&build_ply()).unwrap();
    let t = parse_transforms(TRANSFORMS).unwrap();
    let frame = render(&gaussians, &t.frames[0].c2w, &t.intrinsics, [1.0, 1.0, 1.0], None);
    // 左上角远离三颗高斯 → 纯背景
    assert_eq!(frame.rgb[0], 255);
}
