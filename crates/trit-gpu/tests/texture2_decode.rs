//! TTX2 的 GPU/CPU 解码与 trit-texture 的 CPU 解码逐字节对拍。
//!
//! 与 `texture_decode.rs`（老 TT3，4×10/12 字节）不同：那条传真实块流会在长度
//! 校验上失败并静默 `return`，等于没验证。这条先跑 **CPU 镜像**（无 GPU 也必跑），
//! 钉住「块布局 + 3 进制取位 + 调色板」，再跑 GPU 对拍（无 adapter 才跳过）。

#![cfg(feature = "gpu")]

use trit_gpu::texture2::{
    BLOCK_BYTES, PADDED_BLOCK_BYTES, decode_ttx2_blocks, decode_ttx2_blocks_cpu, padded_words,
};
use trit_texture::{decode_rgba, encode_rgba};

/// 确定性纹理：渐变 + 条纹 + 椒盐，覆盖三种调色板槽位。
fn synthetic(width: usize, height: usize) -> Vec<u8> {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut pixels = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let offset = (y * width + x) * 4;
            pixels[offset] = (x * 255 / width) as u8;
            pixels[offset + 1] = (y * 255 / height) as u8;
            pixels[offset + 2] = ((x / 4 + y / 8) % 2 * 200) as u8;
            pixels[offset + 3] = 255;
            if next() % 16 == 0 {
                let salt = (next() % 256) as u8;
                pixels[offset] = salt;
                pixels[offset + 1] = salt;
                pixels[offset + 2] = salt;
            }
        }
    }
    pixels
}

/// `TritTexture.blocks`（11 字节/块）摊平成字节流。
fn flatten(blocks: &[[u8; BLOCK_BYTES]]) -> Vec<u8> {
    let mut flat = Vec::with_capacity(blocks.len() * BLOCK_BYTES);
    for block in blocks {
        flat.extend_from_slice(block);
    }
    flat
}

#[test]
fn cpu_mirror_matches_trit_texture_byte_for_byte() {
    // CPU 镜像与 WGSL 同算法：这条过了，说明块布局/取位/调色板的**逻辑**是对的，
    // 无 GPU 环境也能守住正确性（GPU 那条可能静默跳过）。
    for (width, height) in [(64usize, 48usize), (4, 8), (128, 64), (256, 8), (4, 32)] {
        let pixels = synthetic(width, height);
        let texture = encode_rgba(&pixels, width, height).unwrap();
        let expected = decode_rgba(&texture).unwrap();
        let flat = flatten(&texture.blocks);
        let got = decode_ttx2_blocks_cpu(&flat, width, height)
            .unwrap_or_else(|| panic!("CPU 镜像应接受 {width}×{height}"));
        assert_eq!(got.len(), expected.len(), "{width}×{height} 长度");
        for (index, (a, b)) in got.iter().zip(expected.iter()).enumerate() {
            assert_eq!(a, b, "{width}×{height} byte {index}");
        }
    }
}

#[test]
fn gpu_decode_matches_cpu_byte_for_byte() {
    let width = 128;
    let height = 64;
    let pixels = synthetic(width, height);
    let texture = encode_rgba(&pixels, width, height).unwrap();
    let cpu = decode_rgba(&texture).unwrap();
    let flat = flatten(&texture.blocks);
    let Some(gpu) = decode_ttx2_blocks(&flat, width, height) else {
        eprintln!("没有可用 GPU adapter，GPU 对拍跳过（CPU 镜像那条仍然必跑）");
        return;
    };
    assert_eq!(gpu.len(), cpu.len(), "GPU 输出长度必须等于 CPU");
    for (index, (gpu_byte, cpu_byte)) in gpu.iter().zip(cpu.iter()).enumerate() {
        assert_eq!(gpu_byte, cpu_byte, "byte {index}");
    }
}

#[test]
fn cpu_mirror_uniform_and_two_color_blocks() {
    // 纯色块：端点退化成同色，三槽全同；双色块：两端点都要零误差还原。
    for (first, second) in [
        ([120u8, 80, 200], None),
        ([40u8, 8, 200], Some([236u8, 251, 250])),
    ] {
        let width = 4;
        let height = 8;
        let mut pixels = vec![0u8; width * height * 4];
        for texel in 0..width * height {
            pixels[texel * 4..texel * 4 + 3].copy_from_slice(&first);
            pixels[texel * 4 + 3] = 255;
        }
        if let Some(second) = second {
            pixels[4..7].copy_from_slice(&second);
        }
        let texture = encode_rgba(&pixels, width, height).unwrap();
        let cpu = decode_rgba(&texture).unwrap();
        let got = decode_ttx2_blocks_cpu(&flatten(&texture.blocks), width, height).unwrap();
        assert_eq!(got, cpu, "纯色/双色块：{first:?} {second:?}");
    }
}

#[test]
fn cpu_mirror_crafted_payload_hits_all_five_trit_slots() {
    // 手工构造单块（4×8）：e0=黑、e1=白，载荷 byte0 = 242 = 2·81+2·27+2·9+2·3+2
    // ⇒ 纹素 0..5 的 trit 全是 2（白），其余 trit=0（黑）。五个槽位（3^0..3^4）
    // 逐一定位——按 2 进制位段误解码会在这里露馅。
    let mut block = vec![0u8; BLOCK_BYTES];
    block[2] = 0xff;
    block[3] = 0xff;
    block[4] = 242;
    let got = decode_ttx2_blocks_cpu(&block, 4, 8).unwrap();
    for texel in 0..5 {
        assert_eq!(
            &got[texel * 4..texel * 4 + 3],
            &[255, 255, 255],
            "texel {texel}（slot {texel}）应为白"
        );
    }
    for texel in 5..32 {
        assert_eq!(
            &got[texel * 4..texel * 4 + 3],
            &[0, 0, 0],
            "texel {texel} 应为黑"
        );
    }
}

#[test]
fn cpu_mirror_mid_palette_entry_is_integer_average() {
    // trit=1 取中点 (e0+e1)/2：黑(0) 与白(255) 的中点是 127（整数除，不是 127.5）。
    let mut block = vec![0u8; BLOCK_BYTES];
    block[2] = 0xff;
    block[3] = 0xff;
    block[4] = 1; // slot0 的 digit = 1 → 中点；其余 slot 为 0 → 黑
    let got = decode_ttx2_blocks_cpu(&block, 4, 8).unwrap();
    assert_eq!(&got[0..3], &[127, 127, 127], "trit=1 必须是整数中点 127");
    assert_eq!(&got[4..7], &[0, 0, 0], "其余纹素应为黑");
}

#[test]
fn geometry_validation_rejects_bad_shapes() {
    let width = 8;
    let height = 16;
    let pixels = synthetic(width, height);
    let texture = encode_rgba(&pixels, width, height).unwrap();
    let flat = flatten(&texture.blocks);
    // 宽不是 4 的倍数 / 高不是 8 的倍数 / 长度不符，都要 None，不能静默产出错图。
    assert!(
        decode_ttx2_blocks_cpu(&flat, 6, height).is_none(),
        "宽非 4 倍数"
    );
    assert!(
        decode_ttx2_blocks_cpu(&flat, width, 12).is_none(),
        "高非 8 倍数"
    );
    assert!(decode_ttx2_blocks_cpu(&flat, 0, height).is_none(), "宽 0");
    assert!(
        decode_ttx2_blocks_cpu(&flat[..flat.len() - 1], width, height).is_none(),
        "长度缺 1 字节"
    );
    assert!(
        decode_ttx2_blocks_cpu(&[0u8; 0], width, height).is_none(),
        "空流"
    );
}

#[test]
fn resident_size_is_about_ten_point_six_seven_times_smaller_than_rgba8() {
    // 显存账：直采样时驻留的是补齐后的块流（12 B / 32 纹素 = 0.375 B/纹素），
    // RGBA8 是 4 B/纹素 ⇒ 10.67×。钉住这个数，以后改布局悄悄退化会立刻红。
    let (width, height) = (256usize, 256usize);
    let pixels = synthetic(width, height);
    let texture = encode_rgba(&pixels, width, height).unwrap();
    let resident = padded_words(&flatten(&texture.blocks)).len() * 4;
    let texels = width * height;
    assert_eq!(resident, texels / 32 * 12, "12 B / 32 纹素");
    let ratio = (texels * 4) as f64 / resident as f64;
    assert!(
        (ratio - 32.0 / 3.0).abs() < 0.01,
        "压缩比实测 {ratio:.3}，应约 10.667×"
    );
}

#[test]
fn padded_words_are_three_per_block_with_zero_pad() {
    // 11 字节/块 → 补齐 12 字节 = 3 个 u32；补的那字节必须是 0（GPU 不读它）。
    let blocks = vec![[0xABu8; BLOCK_BYTES], [0xCDu8; BLOCK_BYTES]];
    let flat = flatten(&blocks);
    let words = padded_words(&flat);
    assert_eq!(words.len(), 6, "两块 → 6 个 u32");
    // 第一块全 0xAB：word0 = 0xABABABAB（LE），word2 末字节是补的 0。
    assert_eq!(words[0], 0xABAB_ABAB, "word0 由 LE 组装");
    assert_eq!(words[2] & 0xFF00_0000, 0, "第 12 字节必须是补零");
    assert_eq!(PADDED_BLOCK_BYTES % 4, 0, "补齐后每块必须是 4 字节的整数倍");
}
