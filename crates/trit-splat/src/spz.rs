//! .spz 资产直读（`spz` crate 0.0.6，feature = "spz"）。
//!
//! SPZ v3 格式（Niantic）：gzip 容器 + 打包二进制——位置 24-bit 定点/f16、
//! scale 8-bit log 编码、rotation smallest-three 10-bit、alpha/color 8-bit、
//! SH 8-bit。解码语义与 Brush PLY 的对应关系：
//! - scales  解码 = log 空间（(packed/16)−10）→ 与 Brush ply 的 log_scale 一致；
//! - alphas  解码 = inv_sigmoid → logit，与 Brush ply 的 opacity 一致；
//! - colors  解码 = (packed/255−0.5)/0.15 → RAW f_dc；
//! - rotations 解码 = xyzw（w 在末位重建）→ 存储时转成 PLY 的 `[W,X,Y,Z]`；
//! - SH      解码 = 逐系数交错 `[r,g,b]` → 重排为 channel-major（Brush ply 口径）；
//! - 坐标系  ：SPZ 容器名义上是 RUB，但 gaussforge 写出的 .spz 保留 Brush PLY
//!   世界系原值（位置 100% 最近邻命中、均值误差 1.17e-4 = 24-bit 定点网格），
//!   因此 unpack 目标取 RUB（恒等），不做 y/z 翻转；
//! - 点序    ：SPZ 按 Morton 序重排，不能按下标与 PLY 对位。
//!
//! 验证口径（2026-11-28，train-brush-001 scene_30000）：
//! - 最近邻抽查 16667 点全部命中，位置 mean |Δ| 1.17e-4 / max 2.07e-4；
//! - scale 差主要来自 SPZ 对 log-scale 的 [-10,10] 截断（首点 -10.76→-10.0）；
//! - 渲染 frame_0016：与 gaussforge 回转 PLY 逐像素一致（∞ dB），
//!   与源 PLY 渲染 32.96 dB；对 GT 27.94 dB（源 PLY 29.48 dB）。

use crate::SplatError;
use crate::ply::RawGaussian;
use spz::CoordinateSystem;
use spz::unpacked::UnpackOptionsBuilder;

/// 读取 .spz 并解码为与 Brush PLY 同语义的原始高斯。
pub fn load_spz(path: &std::path::Path) -> Result<Vec<RawGaussian>, SplatError> {
    let opts = UnpackOptionsBuilder::default()
        .to_coord_system(CoordinateSystem::RUB) // gaussforge .spz = Brush PLY 世界系原值
        .build();
    let gs = spz::GaussianSplat::builder()
        .filepath(path)
        .packed(true)
        .map_err(|e| SplatError::Spz(format!("{e}")))?
        .unpack_options(opts)
        .load()
        .map_err(|e| SplatError::Spz(format!("{e}")))?;

    let n = gs.num_points as usize;
    let deg = gs.spherical_harmonics_degree.clamp(0, 3) as usize;
    let rest_per_channel = match deg {
        0 => 0,
        1 => 3,
        2 => 8,
        _ => 15,
    };
    if gs.positions.len() != n * 3
        || gs.scales.len() != n * 3
        || gs.rotations.len() != n * 4
        || gs.alphas.len() != n
        || gs.colors.len() != n * 3
    {
        return Err(SplatError::Spz("spz 字段长度不一致".into()));
    }
    if rest_per_channel > 0 && gs.spherical_harmonics.len() != n * rest_per_channel * 3 {
        return Err(SplatError::Spz("SH 长度不一致".into()));
    }

    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        // 位置/尺度/logit alpha：与 Brush ply 同空间同激活态
        let position = [
            gs.positions[i * 3],
            gs.positions[i * 3 + 1],
            gs.positions[i * 3 + 2],
        ];
        let scale_log = [gs.scales[i * 3], gs.scales[i * 3 + 1], gs.scales[i * 3 + 2]];
        let opacity_logit = gs.alphas[i];
        // spz 解码四元数是 xyzw → 存储转 PLY 的 [W,X,Y,Z]
        let q = &gs.rotations[i * 4..i * 4 + 4];
        let rot_stored = [q[3], q[0], q[1], q[2]];
        let f_dc = [gs.colors[i * 3], gs.colors[i * 3 + 1], gs.colors[i * 3 + 2]];
        // SH：spz 每点 [coeff·3+channel] 交错 → channel-major [channel·K+coeff]
        let mut f_rest = Vec::with_capacity(rest_per_channel * 3);
        if rest_per_channel > 0 {
            let sh = &gs.spherical_harmonics;
            let base = i * rest_per_channel * 3;
            for c in 0..3 {
                for k in 0..rest_per_channel {
                    f_rest.push(sh[base + k * 3 + c]);
                }
            }
        }
        out.push(RawGaussian {
            position,
            f_dc,
            f_rest,
            opacity_logit,
            scale_log,
            rot_xyzw: rot_stored,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use spz::{GaussianSplat, PackOptions};

    fn write_fixture(path: &std::path::Path) {
        let n = 3usize;
        let k = 3usize; // SH degree 1：每通道 3 个系数
        let mut positions = Vec::new();
        let mut scales = Vec::new();
        let mut rotations = Vec::new();
        let mut alphas = Vec::new();
        let mut colors = Vec::new();
        let mut sh = Vec::new();
        for i in 0..n {
            positions.extend_from_slice(&[
                i as f32 * 1.5 - 1.0,
                i as f32 * -0.75 + 2.0,
                i as f32 * 0.25,
            ]);
            scales.extend_from_slice(&[-2.0 - i as f32 * 0.0625, -1.0, -3.5]);
            rotations.extend_from_slice(&[0.2 + i as f32 * 0.1, 0.3, 0.4, 0.5]);
            alphas.push([0.0f32, 1.0, -2.0][i]);
            colors.extend_from_slice(&[0.0f32, 0.5, -0.5]);
            // 每点/每系数的值都不同：专门抓“所有点都读第 0 点 SH”的越位
            for j in 0..k * 3 {
                sh.push((((i * 9 + j) % 9) as f32 - 4.0) * 0.0625);
            }
        }
        let gs = GaussianSplat {
            num_points: n as i32,
            spherical_harmonics_degree: 1,
            antialiased: false,
            positions,
            scales,
            rotations,
            alphas,
            colors,
            spherical_harmonics: sh,
        };
        // 与 gaussforge/Brush 交付一致：写出的坐标即 Brush PLY 世界系（RUB 恒等）
        let opts = PackOptions::builder().from(CoordinateSystem::RUB).build();
        gs.save_as_packed(path, &opts).expect("写 spz 夹具");
    }

    #[test]
    fn decodes_brush_ply_semantics_and_per_point_sh() {
        let path =
            std::env::temp_dir().join(format!("trit-splat-spz-fixture-{}.spz", std::process::id()));
        write_fixture(&path);
        let g = load_spz(&path).expect("读 spz 夹具");
        std::fs::remove_file(&path).ok();

        assert_eq!(g.len(), 3);
        assert_eq!(g[0].f_rest.len(), 9, "SH1 = 3 系数 × 3 通道");

        for i in 0..3 {
            let pos = [
                i as f32 * 1.5 - 1.0,
                i as f32 * -0.75 + 2.0,
                i as f32 * 0.25,
            ];
            for c in 0..3 {
                assert!((g[i].position[c] - pos[c]).abs() < 1e-3, "position");
                assert!(
                    (g[i].scale_log[c] - [-2.0 - i as f32 * 0.0625, -1.0, -3.5][c]).abs() < 0.02,
                    "scale_log"
                );
                assert!((g[i].f_dc[c] - [0.0f32, 0.5, -0.5][c]).abs() < 0.02, "f_dc");
            }
            let alpha = [0.0f32, 1.0, -2.0][i];
            assert!(
                (g[i].opacity_logit - alpha).abs() < 0.03,
                "opacity_logit: {} vs {alpha}",
                g[i].opacity_logit
            );

            // 旋转：spz 解码 xyzw，存储 [W,X,Y,Z]；q 与 -q 等价
            let q = [0.2 + i as f32 * 0.1, 0.3f32, 0.4, 0.5];
            let norm = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
            let want = [q[3] / norm, q[0] / norm, q[1] / norm, q[2] / norm];
            let got = g[i].rot_xyzw;
            let dot: f32 = (0..4).map(|k| got[k] * want[k]).sum();
            assert!(dot.abs() > 0.999, "rotation dot={dot}");

            for c in 0..3 {
                for k in 0..3 {
                    let want = (((i * 9 + k * 3 + c) % 9) as f32 - 4.0) * 0.0625;
                    assert_eq!(
                        g[i].f_rest[c * 3 + k],
                        want,
                        "point {i} channel {c} coeff {k} SH 越位"
                    );
                }
            }
        }
    }
}
