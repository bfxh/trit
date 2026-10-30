use trit_core::value::Ternary;

use crate::endpoints::{nearest_index, palette_from_endpoints};
use crate::{
    BINARY_BLOCK_BYTES, BLOCK_HEIGHT, BLOCK_PIXELS, BLOCK_WIDTH, MAX_TEXTURE_PIXELS,
    TRIT_BLOCK_BYTES, TRIT_PAYLOAD_BYTES, TRITS_PER_BLOCK, TextureError, TritTexture, decode_rgba,
    decode_rgba_baseline, decode_rgba_into, encode_block, encode_rgba, encode_rgba_baseline,
    pack_rgb565, pack_trits, psnr_rgb, unpack_rgb565, unpack_trits,
};

fn solid(color: [u8; 3]) -> [[u8; 4]; BLOCK_PIXELS] {
    [[color[0], color[1], color[2], 255]; BLOCK_PIXELS]
}

#[test]
fn pack_roundtrips_all_five_trit_values() {
    // 3^5 = 243 ≤ 256：全部 243 种 5-trit 组合都必须逐位还原。
    let mut digits = [Ternary::False; 5];
    for value in 0..243u32 {
        let mut rest = value;
        for digit in digits.iter_mut() {
            *digit = match rest % 3 {
                0 => Ternary::False,
                1 => Ternary::Unknown,
                _ => Ternary::True,
            };
            rest /= 3;
        }
        let packed = pack_trits(&digits);
        assert_eq!(packed[0] as u32, value, "value {value}");
        let unpacked = unpack_trits(&packed);
        assert_eq!(&unpacked[..5], &digits, "value {value}");
    }
}

#[test]
fn block_roundtrips_endpoint_colors_exactly() {
    // RGB565 可精确表示的颜色（R/B 8 的倍数、G 4 的倍数）当端点：
    // 端点色必须零误差往返；中间色是端点平均，只要求落到 mid 槽位。
    // 565 可精确表示的色不止「8 的倍数」（高段的展开带低位回填），所以直接用
    // crate 自己的 pack→unpack 构造可表示色，端点必须零误差往返。
    // 端点是**逐通道** min/max（与 BC1 一致），所以两色必须全通道单调，
    // min/max 才恰好是这两个色本身。
    for raw in [
        ([40u8, 8, 200], [236u8, 251, 250]),
        ([0, 0, 0], [255, 255, 255]),
    ] {
        let expand = |c: [u8; 3]| {
            let (r, g, b) = unpack_rgb565(pack_rgb565(c[0], c[1], c[2]));
            [r, g, b]
        };
        let low = expand(raw.0);
        let high = expand(raw.1);
        let mut texels = solid(low);
        texels[1] = [high[0], high[1], high[2], 255];
        // mid 槽位：端点平均色必须精确落到 index 1（mid 舍入写错会跑偏）。
        let avg = |a: u8, b: u8| ((u16::from(a) + u16::from(b)) / 2) as u8;
        texels[2] = [
            avg(low[0], high[0]),
            avg(low[1], high[1]),
            avg(low[2], high[2]),
            255,
        ];
        let block = encode_block(&texels);
        let e0 = u16::from_be_bytes([block[0], block[1]]);
        let e1 = u16::from_be_bytes([block[2], block[3]]);
        let palette = palette_from_endpoints(e0, e1);
        let mids = [texels[2][0], texels[2][1], texels[2][2]];
        for (texel, color) in [(0, low), (1, high), (2, mids)] {
            let pixel = &texels[texel];
            let decoded = palette[nearest_index(&palette, pixel[0], pixel[1], pixel[2])];
            assert_eq!(
                [decoded.0, decoded.1, decoded.2],
                color[..3],
                "case {raw:?} texel {texel}"
            );
        }
    }
}

#[test]
fn texture_roundtrip_and_block_budgets_are_exact() {
    // 8×8 块：TT3 每块 17 字节（4 端点 + 13 载荷）、基线每块 20 字节（4 + 16）
    // ⇒ 2.125 vs 2.5 bpp。载荷 13 字节 = ceil(64/5)，打包税 1.6%。
    let width = 8;
    let height = 8;
    let mut pixels = vec![0u8; width * height * 4];
    for (index, byte) in pixels.iter_mut().enumerate() {
        *byte = (index % 251) as u8;
    }
    let trit = encode_rgba(&pixels, width, height).unwrap();
    assert_eq!(trit.blocks.len(), 1);
    assert_eq!(trit.blocks[0].len(), TRIT_BLOCK_BYTES);
    assert_eq!(TRIT_BLOCK_BYTES, 17);
    let decoded = decode_rgba(&trit).unwrap();
    assert_eq!(decoded.len(), pixels.len());

    let baseline = encode_rgba_baseline(&pixels, width, height).unwrap();
    assert_eq!(baseline.blocks[0].len(), BINARY_BLOCK_BYTES);
    assert_eq!(BINARY_BLOCK_BYTES, 20);
    let decoded_baseline = decode_rgba_baseline(&baseline).unwrap();
    assert_eq!(decoded_baseline.len(), pixels.len());
    // 两种编码的块数相同，每块 17 vs 20 字节（省 15% 码率）。
    assert_eq!(trit.blocks.len(), baseline.blocks.len());
    assert_eq!(
        20 * trit.blocks.len() * TRIT_BLOCK_BYTES,
        17 * baseline.blocks.len() * BINARY_BLOCK_BYTES
    );
    // 打包税：64 trit 占 13×5 = 65 槽 ⇒ 浪费 1/65 ≈ 1.6%（不是 v1 的 20% 载荷差）。
    assert_eq!(TRIT_PAYLOAD_BYTES * 5 - TRITS_PER_BLOCK, 1);
}

#[test]
fn block_shape_is_mip_friendly() {
    // 回归门：块形状必须都是 2 的幂，否则 mip 链每级减半后很快不合法，
    // 正常 2 的幂纹理就得补边（v1 的 4×10 高须 %10 ⇒ 恒定 25% 填充）。
    //
    // 这正是「零槽位浪费」的陷阱：40 恰被 5 整除看着很美，但 10 不是 2 的幂，
    // 于是每级减半都失去合法性。要把这条钉死，防止有人为了省 1.6% 打包税
    // 把块改回 4×10。
    assert!(
        BLOCK_WIDTH.is_power_of_two(),
        "块宽 {BLOCK_WIDTH} 必须是 2 的幂，否则 mip 链断裂"
    );
    assert!(
        BLOCK_HEIGHT.is_power_of_two(),
        "块高 {BLOCK_HEIGHT} 必须是 2 的幂，否则 mip 链断裂"
    );
    // 方形块：宽高相等时任意 2 的幂纹理都能一路 mip 到 1×1。
    assert_eq!(BLOCK_WIDTH, BLOCK_HEIGHT, "非方块会让 mip 衰减不对称");
    // 逐级减半：从 4096 一路到**块尺寸**（8），每一级都必须是块的整数倍。
    // 低于一个块的层级（4/2/1）在语义上就是空的——mip 链到底了，不是非法。
    let mut side = 4096usize;
    while side >= BLOCK_WIDTH {
        assert!(
            side.is_multiple_of(BLOCK_WIDTH),
            "{side} 不是块宽 {BLOCK_WIDTH} 的倍数，该级 mip 非法"
        );
        side /= 2;
    }
    // 停下时必须恰好是「减半后不再够一个块」，而不是提前卡在某个中间层。
    assert!(
        side < BLOCK_WIDTH,
        "mip 链应在块尺寸处到底，实际停在 {side}"
    );
}

#[test]
fn multi_block_layout_is_not_word_aligned() {
    // 17 字节/块**不是 4 的倍数**：第二个块从字节 17 开始，跨 u32 边界。
    // GPU 侧曾按 `block_id * 3u` 取字（依赖 12 字节的整除关系），改块形状后
    // 会全图错位。这条在 CPU 侧钉住「块起点确实不按字对齐」这个事实，
    // 并用多块纹理验证解码仍逐块正确（块序错位会立刻反映成颜色错乱）。
    const _: () = assert!(!TRIT_BLOCK_BYTES.is_multiple_of(4), "17 字节应非 4 的倍数");
    let width = BLOCK_WIDTH * 3;
    let height = BLOCK_HEIGHT;
    // 三块**横向**排列（纹理是 3 列 × 1 行块），各给一个可区分的纯色；
    // 解码后逐块核对颜色没串位——块起点错位会立刻反映成串色。
    // 颜色必须经 RGB565 可精确表示（否则失败的是量化误差而非块错位）。
    let exact = |c: [u8; 3]| {
        let (r, g, b) = unpack_rgb565(pack_rgb565(c[0], c[1], c[2]));
        [r, g, b]
    };
    let colors = [
        exact([220, 30, 40]),
        exact([30, 220, 40]),
        exact([40, 30, 220]),
    ];
    let mut pixels = vec![0u8; width * height * 4];
    for (block, &color) in colors.iter().enumerate() {
        for y in 0..height {
            for x in 0..BLOCK_WIDTH {
                let offset = (y * width + block * BLOCK_WIDTH + x) * 4;
                pixels[offset] = color[0];
                pixels[offset + 1] = color[1];
                pixels[offset + 2] = color[2];
                pixels[offset + 3] = 255;
            }
        }
    }
    let texture = encode_rgba(&pixels, width, height).unwrap();
    assert_eq!(texture.blocks.len(), 3);
    // 块的字节总长：3 × 17 = 51，确实不按 4 字对齐。
    assert_eq!(texture.blocks.len() * TRIT_BLOCK_BYTES, 51);
    let decoded = decode_rgba(&texture).unwrap();
    for (block, &color) in colors.iter().enumerate() {
        for y in 0..height {
            for x in 0..BLOCK_WIDTH {
                let offset = (y * width + block * BLOCK_WIDTH + x) * 4;
                assert_eq!(
                    [decoded[offset], decoded[offset + 1], decoded[offset + 2]],
                    color,
                    "块 {block} 的 ({x},{y}) 串色了（块起点错位会导致这个）"
                );
            }
        }
    }
}

#[test]
fn baseline_roundtrips_endpoint_colors_exactly() {
    // 二进制基线与 TT3 同端点同调色板，仅索引编码不同——位打包写错会丢色。
    for raw in [
        ([40u8, 8, 200], [236u8, 251, 250]),
        ([0, 0, 0], [255, 255, 255]),
    ] {
        let expand = |c: [u8; 3]| {
            let (r, g, b) = unpack_rgb565(pack_rgb565(c[0], c[1], c[2]));
            [r, g, b]
        };
        let low = expand(raw.0);
        let high = expand(raw.1);
        // low 填满整块，texel 1 放 high：端点才恰好是这两个色。
        let mut pixels = vec![0u8; BLOCK_WIDTH * BLOCK_HEIGHT * 4];
        for texel in 0..BLOCK_PIXELS {
            pixels[texel * 4] = low[0];
            pixels[texel * 4 + 1] = low[1];
            pixels[texel * 4 + 2] = low[2];
            pixels[texel * 4 + 3] = 255;
        }
        pixels[4] = high[0];
        pixels[5] = high[1];
        pixels[6] = high[2];
        pixels[7] = 255;
        let texture = encode_rgba_baseline(&pixels, BLOCK_WIDTH, BLOCK_HEIGHT).unwrap();
        assert_eq!(texture.blocks.len(), 1);
        let decoded = decode_rgba_baseline(&texture).unwrap();
        for (texel, color) in [(0, low), (1, high)] {
            assert_eq!(
                [
                    decoded[texel * 4],
                    decoded[texel * 4 + 1],
                    decoded[texel * 4 + 2]
                ],
                color,
                "case {raw:?} texel {texel}"
            );
        }
    }
}

#[test]
fn rejects_bad_shape_size_and_oversize() {
    assert!(matches!(
        encode_rgba(&[], 3, 8),
        Err(TextureError::Shape {
            width: 3,
            height: 8
        })
    ));
    assert!(matches!(
        encode_rgba(&[0; 64], 4, 4),
        Err(TextureError::Shape {
            width: 4,
            height: 4
        })
    ));
    assert!(matches!(
        encode_rgba(&[0; 16], 8, 8),
        Err(TextureError::SizeMismatch {
            actual: 16,
            expected: 256
        })
    ));
    let big = (MAX_TEXTURE_PIXELS + 1).next_multiple_of(BLOCK_PIXELS);
    assert!(matches!(
        encode_rgba(&[], BLOCK_WIDTH, big / BLOCK_WIDTH),
        Err(TextureError::TooLarge { .. })
    ));
    // 解码侧块数不符。
    let texture = TritTexture {
        width: BLOCK_WIDTH,
        height: BLOCK_HEIGHT,
        blocks: vec![[0u8; TRIT_BLOCK_BYTES]; 3],
    };
    assert!(matches!(
        decode_rgba(&texture),
        Err(TextureError::BlockCount {
            actual: 3,
            expected: 1
        })
    ));
}

#[test]
fn decode_into_matches_allocating_and_rejects_short_buffers() {
    let width = BLOCK_WIDTH;
    let height = BLOCK_HEIGHT;
    let pixels: Vec<u8> = (0..width * height * 4).map(|i| (i % 251) as u8).collect();
    let texture = encode_rgba(&pixels, width, height).unwrap();
    let allocating = decode_rgba(&texture).unwrap();
    let mut reused = vec![7u8; width * height * 4];
    decode_rgba_into(&texture, &mut reused).unwrap();
    assert_eq!(reused, allocating, "into 路径必须与分配路径逐字节一致");
    // alpha 恒 255：两条路径共用实现，必须显式钉住通道值。
    for texel in 0..width * height {
        assert_eq!(reused[texel * 4 + 3], 255, "texel {texel}");
    }
    // 输出缓冲长度不符：显式拒绝而不是 panic。
    let mut short = vec![0u8; width * height * 4 - 4];
    assert!(matches!(
        decode_rgba_into(&texture, &mut short),
        Err(TextureError::SizeMismatch { .. })
    ));
}

#[test]
fn psnr_reports_none_for_identical_and_ranks_clean_over_noise() {
    assert_eq!(psnr_rgb(&[], &[]), None);
    let a = vec![128u8; 64];
    assert_eq!(psnr_rgb(&a, &a), None);
    let clean = vec![100u8; 4];
    let noisy = vec![110u8; 4];
    let worse = vec![140u8; 4];
    let psnr_clean = psnr_rgb(&clean, &noisy).unwrap();
    let psnr_worse = psnr_rgb(&clean, &worse).unwrap();
    assert!(psnr_clean > psnr_worse, "{psnr_clean} vs {psnr_worse}");
}
