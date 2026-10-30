//! `trit-texture` 同机对比表：合成纹理上 TT3（trit 索引）vs 二进制基线
//! （同端点、同 3 级调色板、2 bit 索引），码率与 PSNR 同尺输出。
//!
//! 可复算：`cargo run -p trit-texture`。对比口径见 docs/TERNARY-BOUNDARIES.md §6/§7：
//! 两边同端点、同调色板、同最近色搜索，唯一变量是索引的进制。

use std::time::Instant;

use trit_texture::{
    BINARY_BLOCK_BYTES, BLOCK_HEIGHT, BLOCK_PIXELS, BLOCK_WIDTH, TRIT_BLOCK_BYTES, decode_rgba,
    decode_rgba_baseline, encode_rgba, encode_rgba_baseline, psnr_rgb,
};

/// 128×64 的合成纹理：水平渐变 + 竖条纹 + 少量椒盐噪点，RGBA。
fn synthetic_texture(width: usize, height: usize) -> Vec<u8> {
    let mut pixels = vec![0u8; width * height * 4];
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for y in 0..height {
        for x in 0..width {
            let offset = (y * width + x) * 4;
            let stripe = if (x / 4) % 2 == 0 { 40 } else { 0 };
            let mut r = (x * 255 / width) as u8;
            let mut g = (y * 255 / height) as u8;
            let mut b = 100u8.saturating_add(stripe);
            // ~2% 纹素打椒盐，暴露最近色索引的行为。
            if next() % 100 < 2 {
                let salt = (next() % 256) as u8;
                r = salt;
                g = salt;
                b = salt;
            }
            pixels[offset] = r;
            pixels[offset + 1] = g;
            pixels[offset + 2] = b;
            pixels[offset + 3] = 255;
        }
    }
    pixels
}

fn main() {
    // 2 的幂尺寸：v2 的 8×8 方块能一路 mip 到 1×1，不需要任何补边填充。
    // （v1 的 4×10 块要求高 %10，这种尺寸必须补到 320/640/…，恒定 25% 填充。）
    let width = 128;
    let height = 64;
    let pixels = synthetic_texture(width, height);
    let texels = width * height;
    let raw_bytes = texels * 4;

    let start = Instant::now();
    let trit = encode_rgba(&pixels, width, height).unwrap();
    let trit_encode_us = start.elapsed().as_micros();
    let trit_decoded = decode_rgba(&trit).unwrap();

    let start = Instant::now();
    let binary = encode_rgba_baseline(&pixels, width, height).unwrap();
    let binary_encode_us = start.elapsed().as_micros();
    let binary_decoded = decode_rgba_baseline(&binary).unwrap();

    let trit_bytes = trit.blocks.len() * TRIT_BLOCK_BYTES;
    let binary_bytes = binary.blocks.len() * BINARY_BLOCK_BYTES;

    println!(
        "合成纹理 {width}×{height}（{texels} 纹素，块 = {BLOCK_WIDTH}×{BLOCK_HEIGHT} = {BLOCK_PIXELS} 纹素，RGB565 端点对 + 3 级调色板）"
    );
    println!();
    println!(
        "{:<28} {:>8} {:>8} {:>8} {:>9} {:>9}",
        "format", "bytes", "bpp", "PSNR", "enc µs", "块字节"
    );
    println!(
        "{:<28} {:>8} {:>8} {:>8} {:>9} {:>8}",
        "raw RGBA", raw_bytes, "-", "-", "-", "-"
    );
    println!(
        "{:<28} {:>8} {:>8.2} {:>8.2} {:>9} {:>8}",
        "TT3 (1 trit/texel)",
        trit_bytes,
        trit_bytes as f64 * 8.0 / texels as f64,
        psnr_rgb(&pixels, &trit_decoded).unwrap_or(f64::INFINITY),
        trit_encode_us,
        TRIT_BLOCK_BYTES
    );
    println!(
        "{:<28} {:>8} {:>8.2} {:>8.2} {:>9} {:>8}",
        "binary 2-bit (3-level palette)",
        binary_bytes,
        binary_bytes as f64 * 8.0 / texels as f64,
        psnr_rgb(&pixels, &binary_decoded).unwrap_or(f64::INFINITY),
        binary_encode_us,
        BINARY_BLOCK_BYTES
    );
    println!();
    println!(
        "等调色板（3 级）码率差：每块 {} vs {} 字节（{}%）",
        TRIT_BLOCK_BYTES,
        BINARY_BLOCK_BYTES,
        100 * (BINARY_BLOCK_BYTES - TRIT_BLOCK_BYTES) / BINARY_BLOCK_BYTES
    );
    // 两行对比的是**两个不同维度的差**，必须分开报，不能相减成一个数：
    //   1) 索引进制差（同端点、同调色板）——TT3 相对 2-bit 的 A 类红利，PSNR 差恒为 0；
    //   2) 端点优化差（TT3 用最小二乘精修、基线用 min/max）——D 类编码质量，与进制无关。
    println!(
        "PSNR 增益（TT3 端点最小二乘精修 vs 基线 min/max）：{:+.3} dB",
        psnr_rgb(&pixels, &trit_decoded).unwrap() - psnr_rgb(&pixels, &binary_decoded).unwrap()
    );
    println!(
        "注：索引进制本身不改变画质（同端点同调色板时 PSNR 差为 0）；上面的增益来自端点选择，"
    );
    println!("    字节数不变，属编码质量（D 类），不是三进制红利。");
    println!();
    // mip 链合法性：这是 v2 换块形状的理由，必须可复算地打出来。
    println!("mip 链合法性（每级宽高减半，减半后仍须是块的整数倍）：");
    for (label, bw, bh) in [
        ("v2 8×8 ", BLOCK_WIDTH, BLOCK_HEIGHT),
        ("v1 4×10", 4usize, 10usize),
    ] {
        let mut w = width;
        let mut h = height;
        let mut levels: u32 = 1;
        while w.is_multiple_of(bw * 2) && h.is_multiple_of(bh * 2) {
            w /= 2;
            h /= 2;
            levels += 1;
        }
        // 要拿到 levels 级，原始高度必须是 bh·2^(levels-1) 的倍数；不是就得补边。
        let unit = bh * (1usize << levels.saturating_sub(1));
        let padded = height.next_multiple_of(unit);
        let pad_pct = 100.0 * (padded - height) as f64 / height as f64;
        // 有效码率：补边后多出的纹素也要存，bpp 按**原**纹素算 ⇒ 名义 bpp × 补边比例。
        let nominal = TRIT_BLOCK_BYTES as f64 * 8.0 / BLOCK_PIXELS as f64;
        println!(
            "  {label}: {levels} 级；高度补到 {padded}（+{pad_pct:.1}%）⇒ 有效 {:.3} bpp",
            nominal * padded as f64 / height as f64
        );
    }
}
