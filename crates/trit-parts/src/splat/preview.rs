//! 环绕相机与预览拼图：把一份高斯资产从任意 yaw 重渲成新视角。
//!
//! 相机约定与 [`crate::geo::Camera`] 一致（Blender 式：看 −Z、+Y 朝上）。
//! 这里是**新视角**而不是输入视角：位置/朝向只由场景包围盒 + 仰角/视场角决定，
//! 所以同一份资产在任何机器上都得到同一组图——复核器可以逐像素重算。

use crate::geo::Camera;
use crate::splat::Splat;

/// 包围盒 `(min, max)`。
pub fn bounds(splats: &[Splat]) -> ([f32; 3], [f32; 3]) {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for s in splats {
        for k in 0..3 {
            lo[k] = lo[k].min(s.pos[k]);
            hi[k] = hi[k].max(s.pos[k]);
        }
    }
    (lo, hi)
}

fn unit(v: [f64; 3]) -> [f64; 3] {
    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
    [v[0] / n, v[1] / n, v[2] / n]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// 绕 `center`、半径 `radius`、仰角 `elev` 的相机；`yaw_deg` 是水平角。
/// 视场角 `fov_deg`（水平/垂直相同）与 `res` 决定内参。
pub fn orbit_camera(
    center: [f32; 3],
    radius: f32,
    elev_deg: f32,
    fov_deg: f32,
    res: usize,
    yaw_deg: f32,
) -> Camera {
    let yaw = f64::from(yaw_deg).to_radians();
    let elev = f64::from(elev_deg).to_radians();
    let r = f64::from(radius);
    let eye = [
        f64::from(center[0]) + r * elev.cos() * yaw.cos(),
        f64::from(center[1]) + r * elev.sin(),
        f64::from(center[2]) + r * elev.cos() * yaw.sin(),
    ];
    let tgt = [
        f64::from(center[0]),
        f64::from(center[1]),
        f64::from(center[2]),
    ];
    // 相机 +Z 指向「从目标看向眼睛」的反方向（Blender 看 −Z）
    let z = unit([eye[0] - tgt[0], eye[1] - tgt[1], eye[2] - tgt[2]]);
    let x = unit(cross([0.0, 1.0, 0.0], z));
    let y = cross(z, x);
    let fov = f64::from(fov_deg).to_radians();
    let f = res as f64 / (2.0 * (fov / 2.0).tan());
    let c = res as f64 / 2.0;
    Camera {
        k: [[f, 0.0, c], [0.0, f, c], [0.0, 0.0, 1.0]],
        // `t_cw` 的旋转部分按**列**是相机三轴（project 里用列与世界向量点积）；
        // 平移是相机中心，与 `T_wc` 的语义一致。
        t_cw: [
            [x[0], y[0], z[0], eye[0]],
            [x[1], y[1], z[1], eye[1]],
            [x[2], y[2], z[2], eye[2]],
            [0.0, 0.0, 0.0, 1.0],
        ],
    }
}

/// 把若干张 `res×res×3` 的图拼成 `cols` 列的联系表（行优先）。
pub fn contact_sheet(frames: &[Vec<f32>], res: usize, cols: usize) -> (usize, usize, Vec<f32>) {
    let cols = cols.max(1);
    let rows = frames.len().div_ceil(cols).max(1);
    let (w, h) = (cols * res, rows * res);
    let mut sheet = vec![0.0f32; w * h * 3];
    for (fi, f) in frames.iter().enumerate() {
        if f.len() != res * res * 3 {
            continue;
        }
        let (cx, cy) = (fi % cols, fi / cols);
        for y in 0..res {
            let src = y * res * 3;
            let dst = ((cy * res + y) * w + cx * res) * 3;
            sheet[dst..dst + res * 3].copy_from_slice(&f[src..src + res * 3]);
        }
    }
    (w, h, sheet)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn splats() -> Vec<Splat> {
        vec![
            Splat {
                pos: [-1.0, 0.0, -1.0],
                scale: 0.05,
                color: [1.0, 0.0, 0.0],
                alpha: 0.9,
            },
            Splat {
                pos: [1.0, 2.0, 1.0],
                scale: 0.05,
                color: [0.0, 1.0, 0.0],
                alpha: 0.9,
            },
        ]
    }

    #[test]
    fn bounds_and_center_projection() {
        let (lo, hi) = bounds(&splats());
        assert_eq!(lo, [-1.0, 0.0, -1.0]);
        assert_eq!(hi, [1.0, 2.0, 1.0]);
        let center = [0.0, 1.0, 0.0];
        for yaw in (0..360).step_by(45) {
            let cam = orbit_camera(center, 5.0, 20.0, 40.0, 256, yaw as f32);
            let (u, v, d) = cam.project(center).expect("中心必须在相机前方");
            assert!((u - 128.0).abs() < 1e-6, "yaw {yaw}: u {u}");
            assert!((v - 128.0).abs() < 1e-6, "yaw {yaw}: v {v}");
            assert!((d - 5.0).abs() < 1e-4, "yaw {yaw}: d {d}");
        }
    }

    #[test]
    fn sheet_places_frames_row_major() {
        let red = vec![1.0f32; 4 * 4 * 3];
        let green = vec![0.0f32; 4 * 4 * 3]
            .into_iter()
            .enumerate()
            .map(|(i, _)| if i % 3 == 1 { 1.0 } else { 0.0 })
            .collect::<Vec<_>>();
        let (w, h, s) = contact_sheet(&[red, green], 4, 2);
        assert_eq!((w, h), (8, 4));
        // 左上红、右上绿
        assert_eq!(&s[0..3], &[1.0, 1.0, 1.0]);
        let i = 4 * 3;
        assert_eq!(&s[i..i + 3], &[0.0, 1.0, 0.0]);
    }
}
