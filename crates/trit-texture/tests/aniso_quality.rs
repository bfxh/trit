//! 各向异性过滤的判决性判据：在**强斜视**下，误差必须显著低于纯三线性。
//!
//! 参考真值 = 每个输出像素内 16×16 抖动的点采样平均（= 该像素足迹上的积分）。
//! 若 aniso 与三线性的 RMSE 接近，说明实现没起作用（例如 tap 方向或 lod 取错）。

use trit_texture::convert::rgba_to_ttx2_mips;
use trit_texture::sample::{Wrap as CpuWrap, lod_from_derivatives, sample_aniso};
use trit_texture::ttx2::from_ttx2;

/// 竖向 1 纹素细条纹：沿 x 高频、沿 y 平坦 —— 典型的斜视受害图案。
/// 纹理高 8（只有 1 级 mip）：三线性的 lod 会被钳在 0、按 8 纹素步长欠采样；
/// aniso 沿长轴 8 tap 才能平均回来。
fn stripes(width: usize, height: usize) -> Vec<u8> {
    let mut px = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let o = (y * width + x) * 4;
            let c = if (x / 2) % 2 == 0 { 235 } else { 20 };
            px[o] = c;
            px[o + 1] = c;
            px[o + 2] = c;
            px[o + 3] = 255;
        }
    }
    px
}

/// 像素足迹平均（参考真值）：在屏幕上 16×16 抖动，取 lod=0 的点采样。
fn reference(
    doc: &trit_texture::ttx2::TtxDocument,
    u0: f32,
    v0: f32,
    du: f32,
    dv: f32,
) -> [f32; 3] {
    const N: usize = 16;
    let mut acc = [0f32; 3];
    for j in 0..N {
        for i in 0..N {
            let su = (i as f32 + 0.5) / N as f32;
            let sv = (j as f32 + 0.5) / N as f32;
            let px = doc
                .sample_trilinear(u0 + du * su, v0 + dv * sv, 0.0)
                .expect("sample");
            for (c, slot) in acc.iter_mut().enumerate() {
                *slot += px[c] as f32;
            }
        }
    }
    for slot in &mut acc {
        *slot /= (N * N) as f32;
    }
    acc
}

#[test]
fn aniso_beats_trilinear_on_oblique_minification() {
    let (width, height) = (256usize, 8usize);
    let pixels = stripes(width, height);
    let bytes = rgba_to_ttx2_mips(&pixels, width, height).expect("encode");
    let doc = from_ttx2(&bytes).expect("parse");

    // 屏幕一个像素 = 沿 u 走 8 个纹素、沿 v 走 0.2 个纹素（强各向异性）
    let (w, h) = (24usize, 6usize);
    let du = 8.5 / width as f32; // 步长取 8.5 纹素：落在纹素之间，三线性只做相邻平均（欠采样）
    let dv = 0.02 / height as f32;

    let mut sum_iso = 0f64;
    let mut sum_aniso = 0f64;
    let mut count = 0f64;
    // 边缘像素的足迹会伸到纹理外：aniso 的 tap 走 clamp、参考是未裁剪的，
    // 那属于边界语义而不是过滤质量，跳过一圈边缘再比。
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let u = x as f32 * du;
            let v = 0.5 + y as f32 * dv;
            let truth = reference(&doc, u, v, du, dv);

            // 纯三线性：lod 取导数最大值（GPU 各向同性口径）
            let lod_iso = lod_from_derivatives(width, height, du, 0.0, 0.0, dv);
            let iso = doc.sample_trilinear(u, v, lod_iso).expect("iso");

            // 各向异性：沿长轴 8 tap
            let aniso =
                sample_aniso(&doc, u, v, du, 0.0, 0.0, dv, 8, CpuWrap::Clamp).expect("aniso");

            for c in 0..3 {
                let d_iso = iso[c] as f64 - truth[c] as f64;
                let d_an = aniso[c] as f64 - truth[c] as f64;
                sum_iso += d_iso * d_iso;
                sum_aniso += d_an * d_an;
                count += 1.0;
            }
        }
    }
    let rmse_iso = (sum_iso / count).sqrt();
    let rmse_aniso = (sum_aniso / count).sqrt();
    println!("RMSE 三线性 = {rmse_iso:.2}，aniso(8 tap) = {rmse_aniso:.2}");
    assert!(
        rmse_aniso < rmse_iso * 0.8,
        "aniso 没起作用：RMSE {rmse_aniso:.2} vs 三线性 {rmse_iso:.2}（要求 <80%）"
    );
}

#[test]
fn aniso_degrades_to_trilinear_when_isotropic() {
    let (width, height) = (256usize, 8usize);
    let pixels = stripes(width, height);
    let bytes = rgba_to_ttx2_mips(&pixels, width, height).expect("encode");
    let doc = from_ttx2(&bytes).expect("parse");
    // 两个方向导数相同 ⇒ ratio = 1 ⇒ 退化为一次三线性，结果必须逐位相同
    // 纹素空间里各向同性：du*W == dv*H ⇒ ratio = 1 ⇒ 应退化成一次三线性
    let d = 2.0 / width as f32;
    let dv = d * width as f32 / height as f32;
    let lod = 2.0f32.log2();
    let iso = doc.sample_trilinear(0.37, 0.61, lod).expect("iso");
    let an = sample_aniso(&doc, 0.37, 0.61, d, 0.0, 0.0, dv, 8, CpuWrap::Clamp).expect("aniso");
    assert_eq!(an, iso, "各向同性输入下 aniso 必须与三线性逐位一致");
}
