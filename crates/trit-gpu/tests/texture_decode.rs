//! TT3 的 GPU 解码与 trit-texture 的 CPU 解码逐字节对拍。
//!
//! A16 复审点名「GPU 路径未验证」——这条测试就是补那个口的：同一张纹理，
//! GPU（WGSL `texture.wgsl`）与 CPU（trit-texture::decode_rgba）必须产出
//! 完全相同的 RGBA。没有可用 adapter 时跳过（与 `gpu::run` 同约定）。

#![cfg(feature = "gpu")]

use trit_texture::{decode_rgba, encode_rgba};

/// 每块字节数（v2：8×8 = 64 纹素，4 端点 + 13 载荷）。
const BLOCK_BYTES: usize = 17;
/// 块边长（与 trit-texture 的 BLOCK_WIDTH/BLOCK_HEIGHT 对齐）。
const BLOCK_SIDE: usize = 8;
/// 载荷起始字节（4 字节端点之后）。
const PAYLOAD_START: usize = 4;

/// 256×64 的确定性纹理：渐变 + 条纹 + 椒盐，覆盖三种调色板槽位。
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
            pixels[offset + 2] = ((x / 8 + y / 8) % 2 * 200) as u8;
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

fn flatten(texture: &trit_texture::TritTexture) -> Vec<u8> {
    let mut flat = Vec::with_capacity(texture.blocks.len() * BLOCK_BYTES);
    for block in &texture.blocks {
        flat.extend_from_slice(block);
    }
    flat
}

#[test]
fn gpu_decode_matches_cpu_byte_for_byte() {
    let width = 256;
    let height = 64;
    let pixels = synthetic(width, height);
    let texture = encode_rgba(&pixels, width, height).unwrap();
    let cpu = decode_rgba(&texture).unwrap();

    let flat = flatten(&texture);
    let Some(gpu) = trit_gpu::texture::decode_tt3_blocks(&flat, width, height) else {
        eprintln!("没有可用 GPU adapter，对拍跳过");
        return;
    };
    assert_eq!(
        gpu.len(),
        cpu.len(),
        "GPU 输出长度必须等于 CPU（{width}×{height}×4）"
    );
    // 逐字节对拍：报首个差异的位置，不整段倾倒。
    for (index, (gpu_byte, cpu_byte)) in gpu.iter().zip(cpu.iter()).enumerate() {
        assert_eq!(gpu_byte, cpu_byte, "byte {index}");
    }
}

#[test]
fn gpu_decode_uniform_block() {
    // 纯色块：端点退化成同色，调色板三槽全同——GPU 必须还原原色。
    let width = BLOCK_SIDE;
    let height = BLOCK_SIDE;
    let mut pixels = vec![0u8; width * height * 4];
    for texel in 0..width * height {
        pixels[texel * 4] = 120;
        pixels[texel * 4 + 1] = 80;
        pixels[texel * 4 + 2] = 200;
        pixels[texel * 4 + 3] = 255;
    }
    let texture = encode_rgba(&pixels, width, height).unwrap();
    let cpu = decode_rgba(&texture).unwrap();
    let flat = flatten(&texture);
    let Some(gpu) = trit_gpu::texture::decode_tt3_blocks(&flat, width, height) else {
        eprintln!("没有可用 GPU adapter，对拍跳过");
        return;
    };
    assert_eq!(gpu, cpu, "纯色块（565 量化后两边一致）");
}

#[test]
fn gpu_decode_two_color_block() {
    // 双色块（全通道单调）：两个端点色都必须零误差还原。
    let width = BLOCK_SIDE;
    let height = BLOCK_SIDE;
    let mut pixels = vec![0u8; width * height * 4];
    for texel in 0..width * height {
        pixels[texel * 4] = 40;
        pixels[texel * 4 + 1] = 8;
        pixels[texel * 4 + 2] = 200;
        pixels[texel * 4 + 3] = 255;
    }
    pixels[4] = 236;
    pixels[5] = 251;
    pixels[6] = 250;
    pixels[7] = 255;
    let texture = encode_rgba(&pixels, width, height).unwrap();
    let cpu = decode_rgba(&texture).unwrap();
    let flat = flatten(&texture);
    let Some(gpu) = trit_gpu::texture::decode_tt3_blocks(&flat, width, height) else {
        eprintln!("没有可用 GPU adapter，对拍跳过");
        return;
    };
    assert_eq!(gpu, cpu, "双色块");
}

#[test]
fn gpu_decode_crafted_payload_all_five_slots() {
    // 手工构造：e0=黑、e1=白，payload byte0 = 242 = 2·81+2·27+2·9+2·3+2
    // ⇒ 前 5 个纹素的 trit 全是 2（白），其余纹素 trit=0（黑）。
    // 五个槽位（3^0..3^4）逐一定位——slot 4 曾被按 3-bit 位段误解码。
    let width = BLOCK_SIDE;
    let height = BLOCK_SIDE;
    let mut flat = vec![0u8; BLOCK_BYTES];
    flat[2] = 0xff;
    flat[3] = 0xff;
    flat[PAYLOAD_START] = 242;
    let Some(gpu) = trit_gpu::texture::decode_tt3_blocks(&flat, width, height) else {
        eprintln!("没有可用 GPU adapter，对拍跳过");
        return;
    };
    for texel in 0..5 {
        assert_eq!(
            &gpu[texel * 4..texel * 4 + 3],
            &[255, 255, 255],
            "texel {texel}（slot {texel}）应为白"
        );
    }
    for texel in 5..64 {
        assert_eq!(
            &gpu[texel * 4..texel * 4 + 3],
            &[0, 0, 0],
            "texel {texel} 应为黑"
        );
    }
}
