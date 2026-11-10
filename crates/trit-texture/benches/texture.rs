//! trit-texture 同机读数：TT3 与二进制基线的编/解码吞吐与分配税。
//!
//! 可复算：`cargo bench -p trit-texture --bench texture`。
//! 对比口径：同一张合成纹理，同机同构建；MB/s 按 RGBA 输入/输出字节数计。

use std::hint::black_box;
use std::time::Instant;

// RNG 改吃 trit-fuzz 的 SeedRng（同一条 xorshift 13/7/17 ⇒ 序列不变）。
use trit_fuzz::SeedRng;
use trit_texture::{
    BLOCK_HEIGHT, BLOCK_WIDTH, decode_rgba, decode_rgba_baseline, decode_rgba_into, encode_rgba,
    encode_rgba_baseline,
};

/// 512×256 合成纹理：平滑渐变（可压）+ 椒盐噪声（难压），混合真实感。
fn synthetic(width: usize, height: usize) -> Vec<u8> {
    let mut pixels = vec![0u8; width * height * 4];
    let mut rng = SeedRng::new(0x9E37_79B9_7F4A_7C15);
    for y in 0..height {
        for x in 0..width {
            let offset = (y * width + x) * 4;
            pixels[offset] = (x * 255 / width) as u8;
            pixels[offset + 1] = (y * 255 / height) as u8;
            pixels[offset + 2] = ((x + y) % 64 * 4) as u8;
            pixels[offset + 3] = 255;
            if rng.next_u64().is_multiple_of(64) {
                let salt = (rng.next_u64() % 256) as u8;
                pixels[offset] = salt;
                pixels[offset + 1] = salt;
                pixels[offset + 2] = salt;
            }
        }
    }
    pixels
}

fn bench<T>(name: &str, iters: u32, mut body: impl FnMut() -> T) {
    black_box(body());
    let start = Instant::now();
    for _ in 0..iters {
        black_box(body());
    }
    let per_iter = start.elapsed().as_secs_f64() * 1e6 / f64::from(iters);
    println!("{name:<44} {iters:>6}  {per_iter:>10.2} us/iter");
}

fn main() {
    let width = 512;
    let height = 260;
    assert!(width % BLOCK_WIDTH == 0 && height % BLOCK_HEIGHT == 0);
    let pixels = synthetic(width, height);
    let texels = width * height;
    let input_mb = pixels.len() as f64 / (1 << 20) as f64;
    println!("合成纹理 {width}×{height}（{texels} 纹素，输入 {input_mb:.2} MiB RGBA）");
    println!();

    let iters = 40;
    bench("encode TT3", iters, || {
        encode_rgba(&pixels, width, height).unwrap()
    });
    let trit = encode_rgba(&pixels, width, height).unwrap();
    bench("decode TT3 (allocating)", iters, || {
        decode_rgba(&trit).unwrap()
    });
    let mut reuse = vec![0u8; width * height * 4];
    bench("decode TT3 (into reuse)", iters, || {
        decode_rgba_into(&trit, &mut reuse).unwrap()
    });
    bench("encode binary baseline", iters, || {
        encode_rgba_baseline(&pixels, width, height).unwrap()
    });
    let binary = encode_rgba_baseline(&pixels, width, height).unwrap();
    bench("decode binary baseline", iters, || {
        decode_rgba_baseline(&binary).unwrap()
    });

    // 吞吐换算（单次均值）。
    let t0 = Instant::now();
    for _ in 0..iters {
        black_box(encode_rgba(&pixels, width, height).unwrap());
    }
    let encode_ms = t0.elapsed().as_secs_f64() * 1000.0 / f64::from(iters);
    let t0 = Instant::now();
    for _ in 0..iters {
        black_box(decode_rgba(&trit).unwrap());
    }
    let decode_ms = t0.elapsed().as_secs_f64() * 1000.0 / f64::from(iters);
    println!();
    println!(
        "TT3 吞吐：encode {:>6.0} MB/s（输入），decode {:>6.0} MB/s（输出）",
        input_mb / encode_ms * 1000.0,
        input_mb / decode_ms * 1000.0,
    );
}
