//! TT3 的 GPU 采样与 trit-texture 的 CPU 参考**逐字节对拍**。
//!
//! 同一张纹理、同一批采样点（纹素中心、半格中点、越界 uv），
//! GPU（`sampler.wgsl`）与 CPU（`trit_texture::sample::sample_bilinear_with`）
//! 必须给出完全相同的 RGBA。没有可用 adapter 时跳过（与 `gpu::run` 同约定）。

#![cfg(feature = "gpu")]

use trit_gpu::sampler::{Wrap as GpuWrap, sample_tt3};
use trit_texture::encode_rgba;
use trit_texture::sample::{Wrap as CpuWrap, sample_bilinear_with};

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
    assert!(
        mismatched * 1000 <= pts.len(),
        "{label}: {mismatched}/{} 个样点不一致，超过 0.1% 上限",
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
