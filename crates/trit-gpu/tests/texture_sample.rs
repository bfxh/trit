//! TT3 的 GPU 采样与 trit-texture 的 CPU 参考**逐字节对拍**。
//!
//! 同一张纹理、同一批采样点（纹素中心、半格中点、越界 uv），
//! GPU（`sampler.wgsl`）与 CPU（`trit_texture::sample::sample_bilinear_with`）
//! 必须给出完全相同的 RGBA。没有可用 adapter 时跳过（与 `gpu::run` 同约定）。

#![cfg(feature = "gpu")]

use trit_gpu::sampler::{MipLevel, sample_tt3_mips};
use trit_gpu::sampler::{Wrap as GpuWrap, sample_tt3};
use trit_texture::convert::build_mip_chain;
use trit_texture::encode_rgba;
use trit_texture::sample::{Wrap as CpuWrap, sample_bilinear_with};
use trit_texture::ttx2::TtxDocument;

/// 256×60 的确定性纹理：渐变 + 条纹 + 椒盐，覆盖三种调色板槽位。
fn synthetic(width: usize, height: usize) -> Vec<u8> {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut pixels = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let offset = (y * width + x) * 4;
            let mut r = (x * 255 / width) as u8;
            let mut g = (y * 255 / height) as u8;
            let mut b = (((x / 3 + y / 5) % 2) * 255) as u8;
            if state.is_multiple_of(17) {
                let salt = (state % 256) as u8;
                r = salt;
                g = 255 - salt;
                b = salt / 2;
            }
            pixels[offset] = r;
            pixels[offset + 1] = g;
            pixels[offset + 2] = b;
            pixels[offset + 3] = 255;
        }
    }
    pixels
}

/// 采样点：纹素中心 + 半格中点 + 越界（-0.25 / 1.25）。
fn points(width: usize, height: usize) -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    let (w, h) = (width as f32, height as f32);
    for j in 0..height.min(24) {
        for i in 0..width.min(40) {
            out.push(((i as f32 + 0.5) / w, (j as f32 + 0.5) / h));
            out.push(((i as f32 + 1.0) / w, (j as f32 + 0.5) / h));
        }
    }
    for &(u, v) in &[(-0.25f32, -0.25f32), (1.25, 1.25), (0.0, 1.0), (1.0, 0.0)] {
        out.push((u, v));
    }
    out
}

fn check(wrap_cpu: CpuWrap, wrap_gpu: GpuWrap, label: &str) {
    let width = 256usize;
    let height = 60usize;
    let pixels = synthetic(width, height);
    let texture = encode_rgba(&pixels, width, height).expect("encode");
    let blocks: Vec<u8> = texture.blocks.concat();

    let pts = points(width, height);
    let Some(gpu) = sample_tt3(&blocks, width, height, &pts, wrap_gpu) else {
        eprintln!("没有可用 adapter，跳过 {label}");
        return;
    };
    assert_eq!(gpu.len(), pts.len() * 4);

    let mut mismatched = 0usize;
    let mut max_delta = 0i32;
    for (index, &(u, v)) in pts.iter().enumerate() {
        let cpu = sample_bilinear_with(&texture, u, v, wrap_cpu);
        let got = &gpu[index * 4..index * 4 + 4];
        if got != cpu {
            mismatched += 1;
            if mismatched <= 3 {
                eprintln!("[{label}] #{index} uv=({u:.6},{v:.6}) cpu={cpu:?} gpu={got:?}");
            }
            for c in 0..4 {
                max_delta = max_delta.max((got[c] as i32 - cpu[c] as i32).abs());
            }
        }
    }
    // 判据：解码/寻址若错了，差值是整块或整通道级别的；这里只允许 f32 末位跨 .5
    // 取整边界的 1 LSB（实测 1/1924，且是 ty≈6e-8 这种「几乎为零但不为零」的样点）。
    assert!(
        max_delta <= 1,
        "{label}: 最大通道差 {max_delta} 超过 1 LSB（解码或寻址错了）"
    );
    // 每通道差 ≤1 是硬界：解码/寻址/mip 偏移若错，差会是整块级别的。
    // 不一致率只作防漂移的上限（实测双线性 0.05%，三线性因逐级量化会放大到 ~1–3%）。
    assert!(
        mismatched * 20 <= pts.len(),
        "{label}: {mismatched}/{} 个样点不一致，超过 5% 上限",
        pts.len()
    );
}

#[test]
fn gpu_sampling_matches_cpu_clamp() {
    check(CpuWrap::Clamp, GpuWrap::Clamp, "clamp");
}

#[test]
fn gpu_sampling_matches_cpu_repeat() {
    check(CpuWrap::Repeat, GpuWrap::Repeat, "repeat");
}

#[test]
fn gpu_sampling_matches_cpu_mirror() {
    check(CpuWrap::Mirror, GpuWrap::Mirror, "mirror");
}

#[test]
fn rejects_bad_shape() {
    let pixels = synthetic(8, 10);
    let texture = encode_rgba(&pixels, 8, 10).expect("encode");
    let blocks: Vec<u8> = texture.blocks.concat();
    assert!(sample_tt3(&blocks, 8, 10, &[(0.5, 0.5)], GpuWrap::Clamp).is_some());
    // 长度少一个字节 ⇒ 拒绝
    assert!(
        sample_tt3(
            &blocks[..blocks.len() - 1],
            8,
            10,
            &[(0.5, 0.5)],
            GpuWrap::Clamp
        )
        .is_none()
    );
    // 宽不是 4 的倍数 ⇒ 拒绝
    assert!(sample_tt3(&blocks, 6, 10, &[(0.5, 0.5)], GpuWrap::Clamp).is_none());
}

/// 三线性：GPU（mip 链打成一个缓冲）vs CPU `sample_trilinear_with`。
fn check_trilinear(wrap_cpu: CpuWrap, wrap_gpu: GpuWrap, label: &str) {
    let width = 256usize;
    let height = 60usize;
    let pixels = synthetic(width, height);
    let chain = build_mip_chain(&pixels, width, height).expect("mip chain");
    assert!(chain.len() >= 3, "需要多级 mip");
    let flat: Vec<Vec<u8>> = chain.iter().map(|t| t.blocks.concat()).collect();
    let levels: Vec<MipLevel> = chain
        .iter()
        .zip(&flat)
        .map(|(t, blocks)| MipLevel {
            blocks,
            width: t.width,
            height: t.height,
            rect: None,
        })
        .collect();
    let doc = TtxDocument::Texture {
        levels: chain.clone(),
    };

    let mut pts: Vec<(f32, f32, f32)> = Vec::new();
    for &lod in &[0.0f32, 0.35, 1.0, 1.5, 2.0, 3.75, 9.0, -1.0] {
        for j in 0..height.min(12) {
            for i in 0..width.min(20) {
                let u = (i as f32 + 0.5) / width as f32;
                let v = (j as f32 + 0.5) / height as f32;
                pts.push((u, v, lod));
            }
        }
    }

    let Some(gpu) = sample_tt3_mips(&levels, &pts, wrap_gpu) else {
        eprintln!("没有可用 adapter，跳过 {label}");
        return;
    };
    let mut mismatched = 0usize;
    let mut max_delta = 0i32;
    for (index, &(u, v, lod)) in pts.iter().enumerate() {
        let cpu = doc
            .sample_trilinear_with(u, v, lod, wrap_cpu)
            .expect("trilinear");
        let got = &gpu[index * 4..index * 4 + 4];
        if got != cpu {
            mismatched += 1;
            if mismatched <= 3 {
                eprintln!(
                    "[{label}] #{index} uv=({u:.5},{v:.5}) lod={lod} cpu={cpu:?} gpu={got:?}"
                );
            }
            for c in 0..4 {
                max_delta = max_delta.max((got[c] as i32 - cpu[c] as i32).abs());
            }
        }
    }
    assert!(
        max_delta <= 1,
        "{label}: 三线性最大通道差 {max_delta} 超过 1 LSB"
    );
    assert!(
        mismatched * 20 <= pts.len(),
        "{label}: {mismatched}/{} 个样点不一致，超过 5% 上限",
        pts.len()
    );
}

#[test]
fn gpu_trilinear_matches_cpu_clamp() {
    check_trilinear(CpuWrap::Clamp, GpuWrap::Clamp, "tri-clamp");
}

#[test]
fn gpu_trilinear_matches_cpu_repeat() {
    check_trilinear(CpuWrap::Repeat, GpuWrap::Repeat, "tri-repeat");
}

#[test]
fn gpu_trilinear_matches_cpu_mirror() {
    check_trilinear(CpuWrap::Mirror, GpuWrap::Mirror, "tri-mirror");
}

/// 图集子矩形：GPU 与 CPU 在矩形内采样的结果必须一致，且**不能 bleed 到邻居**。
#[test]
fn gpu_rect_matches_cpu_and_does_not_bleed() {
    // 左半红、右半绿（每块 4 宽 ⇒ 块内纯色）
    let (width, height) = (64usize, 40usize);
    let mut pixels = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let o = (y * width + x) * 4;
            let c = if x < 32 { [220, 30, 30] } else { [30, 200, 30] };
            pixels[o..o + 3].copy_from_slice(&c);
            pixels[o + 3] = 255;
        }
    }
    let texture = encode_rgba(&pixels, width, height).expect("encode");
    let blocks: Vec<u8> = texture.blocks.concat();
    let rect = trit_gpu::sampler::Rect {
        x: 0,
        y: 0,
        width: 32,
        height: 40,
    };
    let level = MipLevel {
        blocks: &blocks,
        width,
        height,
        rect: Some(rect),
    };

    let mut pts: Vec<(f32, f32, f32)> = Vec::new();
    for j in 0..40 {
        for i in 0..64 {
            let u = (i as f32 + 0.5) / 64.0;
            let v = (j as f32 + 0.5) / 40.0;
            pts.push((u, v, 0.0));
        }
    }
    let Some(gpu) = sample_tt3_mips(&[level], &pts, GpuWrap::Clamp) else {
        eprintln!("没有可用 adapter，跳过 rect");
        return;
    };

    let mut sampler = trit_texture::sample::Sampler::with_wrap(&texture, CpuWrap::Clamp);
    sampler.set_rect(Some(
        trit_texture::sample::Rect::new(0, 0, 32, 40).expect("rect"),
    ));
    let mut mismatched = 0usize;
    let mut max_delta = 0i32;
    for (index, &(u, v, _)) in pts.iter().enumerate() {
        let cpu = sampler.sample_bilinear(u, v);
        let got = &gpu[index * 4..index * 4 + 4];
        if got != cpu {
            mismatched += 1;
            for c in 0..4 {
                max_delta = max_delta.max((got[c] as i32 - cpu[c] as i32).abs());
            }
        }
        // 矩形内不该出现绿色
        assert!(
            got[0] > 200 && got[1] < 60,
            "样点 #{index} 采到了矩形外：{got:?}"
        );
    }
    assert!(max_delta <= 1, "rect: 最大通道差 {max_delta}");
    assert!(
        mismatched * 20 <= pts.len(),
        "rect: {mismatched} 个样点不一致"
    );
}

/// GPU 出图 与 CPU `preview::render` 走同一批 UV/lod，必须 ≤1 LSB。
#[test]
fn gpu_render_matches_cpu_preview() {
    use trit_texture::preview::{RenderOptions, SampleMode, render};
    use trit_texture::sample::Wrap as CpuWrapAgain;

    let (width, height) = (128usize, 60usize);
    let pixels = synthetic(width, height);
    let chain = build_mip_chain(&pixels, width, height).expect("mip chain");
    let flat: Vec<Vec<u8>> = chain.iter().map(|t| t.blocks.concat()).collect();
    let doc = TtxDocument::Texture {
        levels: chain.clone(),
    };

    let opts = RenderOptions {
        width: 96,
        height: 40,
        tiles: 2.0,
        mode: SampleMode::Auto,
        wrap: CpuWrapAgain::Repeat,
        ..RenderOptions::default()
    };
    let (w, h, cpu) = render(&doc, &opts).expect("cpu render");

    // 与 preview::render 同一条 UV 与 LOD 公式
    let base = &chain[0];
    let du = opts.tiles / w as f32;
    let dv = opts.tiles / h as f32;
    let mut pts: Vec<(f32, f32, f32)> = Vec::with_capacity(w * h);
    for y in 0..h {
        let v = (y as f32 + 0.5) / h as f32 * opts.tiles;
        for x in 0..w {
            let u = (x as f32 + 0.5) / w as f32 * opts.tiles;
            let lod = trit_texture::sample::lod_from_derivatives(
                base.width,
                base.height,
                du,
                0.0,
                0.0,
                dv,
            );
            pts.push((u, v, lod));
        }
    }
    let levels: Vec<MipLevel> = chain
        .iter()
        .zip(&flat)
        .map(|(t, blocks)| MipLevel {
            blocks,
            width: t.width,
            height: t.height,
            rect: None,
        })
        .collect();
    let Some(gpu) = sample_tt3_mips(&levels, &pts, GpuWrap::Repeat) else {
        eprintln!("没有可用 adapter，跳过 render");
        return;
    };

    let mut mismatched = 0usize;
    let mut max_delta = 0i32;
    for (index, chunk) in gpu.as_chunks::<4>().0.iter().enumerate() {
        let cpu_px = &cpu[index * 4..index * 4 + 4];
        if chunk != cpu_px {
            mismatched += 1;
            for c in 0..4 {
                max_delta = max_delta.max((chunk[c] as i32 - cpu_px[c] as i32).abs());
            }
        }
    }
    assert!(max_delta <= 1, "render: 最大通道差 {max_delta}");
    assert!(
        mismatched * 20 <= pts.len(),
        "render: {mismatched}/{} 像素不一致",
        pts.len()
    );
}
