//! nerfstudio `transforms.json`（OPENCV 内参 + OpenGL camera-to-world）→ pinhole 相机。
//!
//! 约定与数据集契约一致：`transform_matrix` 是 Blender/OpenGL 的 c2w
//! （+X 右 +Y 上 −Z 前）；`camera_model: OPENCV` 只描述内参口径
//! （u = fl_x·x/(−z) + cx）。深度取 `t = −z_cam > 0`。

use crate::math::Mat4;
use crate::SplatError;
use serde_json::Value;

#[derive(Clone, Copy, Debug)]
pub struct Intrinsics {
    pub fl_x: f32,
    pub fl_y: f32,
    pub cx: f32,
    pub cy: f32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug)]
pub struct CameraFrame {
    pub file_path: String,
    pub c2w: Mat4,
}

#[derive(Clone, Debug)]
pub struct Transforms {
    pub intrinsics: Intrinsics,
    pub frames: Vec<CameraFrame>,
}

fn num(value: &Value, key: &str) -> Result<f32, SplatError> {
    value
        .get(key)
        .and_then(|v| v.as_f64())
        .map(|v| v as f32)
        .ok_or_else(|| SplatError::Transforms(format!("缺字段 {key}")))
}

/// 解析 transforms.json。帧内 w/h/fl 可覆盖顶层（nerfstudio 变体），缺省用顶层。
pub fn parse_transforms(text: &str) -> Result<Transforms, SplatError> {
    let root: Value =
        serde_json::from_str(text).map_err(|e| SplatError::Transforms(e.to_string()))?;
    let width = num(&root, "w")? as u32;
    let height = num(&root, "h")? as u32;
    let frames_value = root
        .get("frames")
        .and_then(|v| v.as_array())
        .ok_or_else(|| SplatError::Transforms("缺 frames 数组".into()))?;

    let mut frames = Vec::with_capacity(frames_value.len());
    for frame in frames_value {
        let matrix = frame
            .get("transform_matrix")
            .and_then(|v| v.as_array())
            .ok_or_else(|| SplatError::Transforms("帧缺 transform_matrix".into()))?;
        let mut flat = [0.0f32; 16];
        let mut k = 0usize;
        for row in matrix {
            for cell in row.as_array().ok_or_else(|| {
                SplatError::Transforms("transform_matrix 行不是数组".into())
            })? {
                if k >= 16 {
                    return Err(SplatError::Transforms("transform_matrix 超 4×4".into()));
                }
                flat[k] = cell.as_f64().unwrap_or(0.0) as f32;
                k += 1;
            }
        }
        if k != 16 {
            return Err(SplatError::Transforms("transform_matrix 不足 4×4".into()));
        }
        frames.push(CameraFrame {
            file_path: frame
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned(),
            c2w: Mat4::from_rows_flat(flat),
        });
    }
    if frames.is_empty() {
        return Err(SplatError::Transforms("frames 为空".into()));
    }

    // 首帧优先取帧内内参（Brush 导出把 fl/cx/cy 平铺进每帧）
    let first = &frames_value[0];
    let fl_x = first.get("fl_x").and_then(|v| v.as_f64()).map(|v| v as f32);
    let intrinsics = match fl_x {
        Some(fl_x) => Intrinsics {
            fl_x,
            fl_y: first.get("fl_y").and_then(|v| v.as_f64()).unwrap_or(fl_x as f64) as f32,
            cx: first.get("cx").and_then(|v| v.as_f64()).unwrap_or(width as f64 / 2.0) as f32,
            cy: first.get("cy").and_then(|v| v.as_f64()).unwrap_or(height as f64 / 2.0) as f32,
            width: first.get("w").and_then(|v| v.as_f64()).unwrap_or(width as f64) as u32,
            height: first.get("h").and_then(|v| v.as_f64()).unwrap_or(height as f64) as u32,
        },
        None => Intrinsics {
            fl_x: num(&root, "fl_x")?,
            fl_y: root.get("fl_y").and_then(|v| v.as_f64()).unwrap_or(
                num(&root, "fl_x")? as f64,
            ) as f32,
            cx: root
                .get("cx")
                .and_then(|v| v.as_f64())
                .unwrap_or(width as f64 / 2.0) as f32,
            cy: root
                .get("cy")
                .and_then(|v| v.as_f64())
                .unwrap_or(height as f64 / 2.0) as f32,
            width,
            height,
        },
    };
    Ok(Transforms { intrinsics, frames })
}

/// 屏幕投影：OpenGL 相机空间（−Z 前，+Y 上）→ OpenCV 像素坐标（v 向下）。
/// 返回 (u, v, 深度 t)。v 的 − 号来自"相机 +Y 朝上 vs 图像 v 朝下"。
pub fn project_to_pixel(
    intr: &Intrinsics,
    c2w: &Mat4,
    world: crate::math::Vec3,
) -> (f32, f32, f32) {
    let cam = c2w.world_to_cam(world);
    let depth = -cam.z; // −Z 前 → 正深度
    let u = intr.fl_x * (cam.x / depth) + intr.cx;
    let v = intr.cy - intr.fl_y * (cam.y / depth);
    (u, v, depth)
}

#[cfg(test)]
mod tests {
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
        assert!(
            (tr.z - 5.0).abs() < 1e-5,
            "平移应为 (0,2,5)，实际 {tr:?}"
        );
    }

    #[test]
    fn point_on_optical_axis_maps_to_principal_point() {
        let t = parse_transforms(SAMPLE).unwrap();
        // 相机在 (0,2,5) 水平看 −Z：正前方 5m 的点 (0,2,0) 在光轴上 → 主点
        let (u, v, depth) =
            project_to_pixel(&t.intrinsics, &t.frames[0].c2w, Vec3::new(0.0, 2.0, 0.0));
        assert!((u - 480.0).abs() < 1e-3, "u={u} v={v} depth={depth}");
        assert!((v - 270.0).abs() < 1e-3, "u={u} v={v} depth={depth}");
        assert!((depth - 5.0).abs() < 1e-4, "u={u} v={v} depth={depth}");
    }

    #[test]
    fn offset_point_displaces_right_and_up_correctly() {
        let t = parse_transforms(SAMPLE).unwrap();
        // 右 1m：(1,2,0) → u = 480 + 1120/5
        let (u, v, depth) =
            project_to_pixel(&t.intrinsics, &t.frames[0].c2w, Vec3::new(1.0, 2.0, 0.0));
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
}
