//! 端到端小场景的共享夹具：内存 PLY + transforms.json。

pub const TRANSFORMS: &str = r#"{
    "camera_model": "OPENCV", "fl_x": 50.0, "fl_y": 50.0,
    "cx": 16.0, "cy": 12.0, "w": 32, "h": 24,
    "frames": [{"file_path": "images/frame_0000.png",
                "transform_matrix": [[1,0,0,0],[0,1,0,0],[0,0,1,5],[0,0,0,1]]}]
}"#;

pub fn build_ply() -> Vec<u8> {
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
            pos[0],
            pos[1],
            pos[2],
            dc[0],
            dc[1],
            dc[2],
            1e8, // opacity logit → sigmoid ≈ 1
            0.3f32.ln(),
            0.3f32.ln(),
            0.3f32.ln(),
            0.0,
            0.0,
            0.0,
            1.0, // rot xyzw = 单位旋转
        ];
        for f in vals {
            out.extend_from_slice(&f.to_le_bytes());
        }
    }
    out
}
