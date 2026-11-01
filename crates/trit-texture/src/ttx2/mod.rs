//! **TTX2**——多载荷容器：kind=TEX（纹理 + mip 链）/ kind=MOD（三值模型）。
//!
//! 布局与竞争力对比见 `docs/TTX.md`；校验哲学与 TTX1 相同：GF(3) CRC 按
//! **内容自己的基数**算，逐段验证，任何不一致都是结构化错误。TTX2 的校验尾
//! 是 **GF(3) CRC-16**（生成多项式 x¹⁶+x⁷+2 本原，≤16 连续 trit 突发错全检）。
//!
//! 模型域（三值层/基 3 打包/层间衔接/GPU 双轨转换）在私有 `model` 子模块，
//! 类型经本模块再导出。

mod model;

pub use model::{TRITS_PER_WORD, TritModel, TritModelLayer};

use trit_core::value::Ternary;

use crate::ttx::{block_trits, gf3_remainder_16};
use crate::{BLOCK_HEIGHT, BLOCK_WIDTH, TRIT_BLOCK_BYTES, TextureError, TritTexture};
use model::{TRITS_PER_PACKED_BYTE, model_trits, pack_radix3, unpack_radix3};

/// 容器魔数。
pub const MAGIC: [u8; 4] = *b"TTX2";
/// 定头字节数（magic + kind + flags + reserved + section_len）。
pub const HEADER_BYTES: usize = 12;
/// 校验尾字节数：GF(3) CRC-16，16 trit 打进 4 字节小端。
pub const CHECKSUM_BYTES: usize = 4;
/// kind：纹理 + mip 链。
pub const KIND_TEX: u8 = 0;
/// kind：三值模型（基 3 打包权重）。
pub const KIND_MOD: u8 = 1;
/// `flags` 的布局位：置位表示 8×8 块（17 字节）。旧文件该位为 0（4×10 / 12 字节），
/// 读到时显式报 `UnsupportedLayout`，而不是走到块数/校验和才炸。
pub const FLAG_LAYOUT_8X8: u8 = 0x01;

/// 一份 TTX2 文档。
#[derive(Debug, Clone, PartialEq)]
pub enum TtxDocument {
    /// 纹理 + mip 链（≥1 级；每级宽高不增且至少一维严格递减）。
    Texture {
        /// 从细到粗的各级纹理。
        levels: Vec<TritTexture>,
    },
    /// 三值模型（线性层链，层间衔接强制；GPU 经 `to_dual_rail` 转换）。
    Model(TritModel),
}

/// TEX 的规范 trit 流：各级全部索引 trit 依序拼接。
fn texture_trits(levels: &[TritTexture]) -> Vec<Ternary> {
    let mut trits = Vec::new();
    for level in levels {
        for block in &level.blocks {
            trits.extend_from_slice(&block_trits(block));
        }
    }
    trits
}

fn level_blocks(width: usize, height: usize) -> Option<usize> {
    if width == 0
        || height == 0
        || !width.is_multiple_of(BLOCK_WIDTH)
        || !height.is_multiple_of(BLOCK_HEIGHT)
    {
        return None;
    }
    (width / BLOCK_WIDTH)
        .checked_mul(height / BLOCK_HEIGHT)
        // checked 内层乘：32 位目标上 usize=u32，未检查的 blocks×12 会回绕
        // 放行超大声明，后续切片越界 panic。
        .filter(|blocks| {
            blocks
                .checked_mul(TRIT_BLOCK_BYTES)
                .is_some_and(|bytes| bytes <= u32::MAX as usize)
        })
}

fn validate_levels(levels: &[TritTexture]) -> Result<(), TextureError> {
    if levels.is_empty() {
        return Err(TextureError::TtxEmpty);
    }
    let mut previous: Option<(usize, usize)> = None;
    for (level, texture) in levels.iter().enumerate() {
        let expected = level_blocks(texture.width, texture.height).ok_or(TextureError::Shape {
            width: texture.width,
            height: texture.height,
        })?;
        if texture.blocks.len() != expected {
            return Err(TextureError::BlockCount {
                actual: texture.blocks.len(),
                expected,
            });
        }
        if let Some((prev_w, prev_h)) = previous {
            let (w, h) = (texture.width, texture.height);
            if !(w <= prev_w && h <= prev_h && (w < prev_w || h < prev_h)) {
                return Err(TextureError::TtxMip { level });
            }
        }
        previous = Some((texture.width, texture.height));
    }
    Ok(())
}

fn encode_texture_section(levels: &[TritTexture]) -> Vec<u8> {
    let mut section = Vec::new();
    section.extend_from_slice(&(levels.len() as u32).to_le_bytes());
    for level in levels {
        section.extend_from_slice(&(level.width as u32).to_le_bytes());
        section.extend_from_slice(&(level.height as u32).to_le_bytes());
        for block in &level.blocks {
            section.extend_from_slice(block);
        }
    }
    section
}

fn encode_model_section(model: &TritModel) -> Vec<u8> {
    let mut section = Vec::new();
    section.extend_from_slice(&(model.layers.len() as u32).to_le_bytes());
    for layer in &model.layers {
        section.extend_from_slice(&(layer.inputs as u32).to_le_bytes());
        section.extend_from_slice(&(layer.outputs as u32).to_le_bytes());
        section.extend_from_slice(&pack_radix3(&layer.weights));
        section.extend_from_slice(&pack_radix3(&layer.bias));
    }
    section
}

/// 序列化成 TTX2 容器（先校验，坏形状不写盘）。
pub fn to_ttx2(document: &TtxDocument) -> Result<Vec<u8>, TextureError> {
    let (kind, section, checksum_trits) = match document {
        TtxDocument::Texture { levels } => {
            validate_levels(levels)?;
            (
                KIND_TEX,
                encode_texture_section(levels),
                texture_trits(levels),
            )
        }
        TtxDocument::Model(model) => {
            TritModel::new(model.layers.clone())?;
            (KIND_MOD, encode_model_section(model), model_trits(model))
        }
    };
    let mut out = Vec::with_capacity(HEADER_BYTES + section.len() + CHECKSUM_BYTES);
    out.extend_from_slice(&MAGIC);
    out.push(kind);
    out.push(FLAG_LAYOUT_8X8);
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(section.len() as u32).to_le_bytes());
    out.extend_from_slice(&section);
    out.extend_from_slice(&gf3_remainder_16(&checksum_trits));
    Ok(out)
}

fn decode_texture_section(section: &[u8]) -> Result<Vec<TritTexture>, TextureError> {
    let truncated = || TextureError::TtxTruncated {
        actual: section.len(),
    };
    if section.len() < 4 {
        return Err(truncated());
    }
    let level_count = u32::from_le_bytes(section[0..4].try_into().unwrap());
    if level_count == 0 {
        return Err(TextureError::TtxEmpty);
    }
    let mut cursor = 4usize;
    let mut levels: Vec<TritTexture> = Vec::new();
    for _ in 0..level_count {
        let head_end = cursor.checked_add(8).ok_or(TextureError::TtxTruncated {
            actual: section.len(),
        })?;
        if head_end > section.len() {
            return Err(truncated());
        }
        let width = u32::from_le_bytes(section[cursor..cursor + 4].try_into().unwrap()) as usize;
        let height =
            u32::from_le_bytes(section[cursor + 4..cursor + 8].try_into().unwrap()) as usize;
        cursor = head_end;
        let blocks = level_blocks(width, height).ok_or(TextureError::Shape { width, height })?;
        let payload = blocks * TRIT_BLOCK_BYTES;
        let end = cursor
            .checked_add(payload)
            .ok_or(TextureError::TtxTruncated {
                actual: section.len(),
            })?;
        if end > section.len() {
            return Err(truncated());
        }
        let mut block_list = Vec::with_capacity(blocks);
        for index in 0..blocks {
            let start = cursor + index * TRIT_BLOCK_BYTES;
            let mut block = [0u8; TRIT_BLOCK_BYTES];
            block.copy_from_slice(&section[start..start + TRIT_BLOCK_BYTES]);
            block_list.push(block);
        }
        cursor = end;
        levels.push(TritTexture {
            width,
            height,
            blocks: block_list,
        });
    }
    // mip 单调性与形状对账单一来源：与写侧共用 validate_levels。
    validate_levels(&levels)?;
    Ok(levels)
}

fn decode_model_section(section: &[u8]) -> Result<TritModel, TextureError> {
    let truncated = || TextureError::TtxTruncated {
        actual: section.len(),
    };
    let mut cursor = 0usize;
    let take_u32 = |cursor: &mut usize, section: &[u8]| -> Option<u32> {
        if *cursor + 4 > section.len() {
            return None;
        }
        let value = u32::from_le_bytes(section[*cursor..*cursor + 4].try_into().unwrap());
        *cursor += 4;
        Some(value)
    };
    let layer_count = take_u32(&mut cursor, section).ok_or_else(truncated)?;
    if layer_count == 0 {
        return Err(TextureError::TtxEmpty);
    }
    let mut layers = Vec::new();
    for _ in 0..layer_count {
        let inputs = take_u32(&mut cursor, section).ok_or_else(truncated)? as usize;
        let outputs = take_u32(&mut cursor, section).ok_or_else(truncated)? as usize;
        if inputs == 0 || outputs == 0 {
            return Err(TextureError::TtxModelShape {
                layer: 0,
                message: "inputs/outputs 必须非零".to_string(),
            });
        }
        let count = outputs
            .checked_mul(inputs)
            .ok_or(TextureError::TtxModelShape {
                layer: 0,
                message: "outputs × inputs 溢出".to_string(),
            })?;
        let weight_bytes = count.div_ceil(TRITS_PER_PACKED_BYTE);
        let bias_bytes = outputs.div_ceil(TRITS_PER_PACKED_BYTE);
        let payload = weight_bytes
            .checked_add(bias_bytes)
            .and_then(|bytes| cursor.checked_add(bytes))
            .ok_or_else(truncated)?;
        if payload > section.len() {
            return Err(truncated());
        }
        let weights = unpack_radix3(&section[cursor..cursor + weight_bytes], count);
        let bias = unpack_radix3(&section[cursor + weight_bytes..payload], outputs);
        cursor = payload;
        layers.push(TritModelLayer {
            inputs,
            outputs,
            weights,
            bias,
        });
    }
    // 层间衔接与取值域单一来源：与写侧共用 TritModel::new。
    TritModel::new(layers)
}

/// 从 TTX2 容器加载：逐段校验（魔数/kind/flags/段长/形状/层间衔接/GF(3) CRC-16）。
///
/// TTX1 文件会得到 `TtxMagic(['T','T','X','1'])`——v2 读 v1 是显式拒绝，
/// 读 v1 请用 [`crate::ttx::from_ttx`]。
pub fn from_ttx2(bytes: &[u8]) -> Result<TtxDocument, TextureError> {
    if bytes.len() < HEADER_BYTES + CHECKSUM_BYTES {
        return Err(TextureError::TtxTruncated {
            actual: bytes.len(),
        });
    }
    if bytes[..4] != MAGIC {
        return Err(TextureError::TtxMagic(bytes[..4].try_into().unwrap()));
    }
    let kind = bytes[4];
    let flags = bytes[5];
    let reserved = u16::from_le_bytes(bytes[6..8].try_into().unwrap());
    if kind != KIND_TEX && kind != KIND_MOD {
        return Err(TextureError::TtxKind(kind));
    }
    if flags == 0 {
        // 旧 4×10 布局：头部本身能区分，不必等到块数/校验和才报错。
        return Err(TextureError::UnsupportedLayout { rev: 0 });
    }
    if flags & !FLAG_LAYOUT_8X8 != 0 || reserved != 0 {
        return Err(TextureError::TtxFlags { flags, reserved });
    }
    let section_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let section_end = HEADER_BYTES
        .checked_add(section_len)
        .and_then(|end| end.checked_add(CHECKSUM_BYTES))
        .ok_or(TextureError::TtxTruncated {
            actual: bytes.len(),
        })?;
    if bytes.len() != section_end {
        return Err(TextureError::TtxTruncated {
            actual: bytes.len(),
        });
    }
    let section = &bytes[HEADER_BYTES..HEADER_BYTES + section_len];
    let document = match kind {
        KIND_TEX => TtxDocument::Texture {
            levels: decode_texture_section(section)?,
        },
        _ => TtxDocument::Model(decode_model_section(section)?),
    };
    let trits = match &document {
        TtxDocument::Texture { levels } => texture_trits(levels),
        TtxDocument::Model(model) => model_trits(model),
    };
    let stored: [u8; CHECKSUM_BYTES] = bytes[HEADER_BYTES + section_len..section_end]
        .try_into()
        .unwrap();
    let expected = gf3_remainder_16(&trits);
    if stored != expected {
        return Err(TextureError::TtxChecksum {
            stored: stored[0],
            expected: expected[0],
        });
    }
    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ttx;
    use crate::{decode_rgba, encode_rgba};

    fn sample(width: usize, height: usize) -> Vec<u8> {
        (0..width * height * 4).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn model_container_rejects_absurd_dims_without_allocation() {
        // 谎报 outputs 巨大：载荷按 checked 对账后拒成 TtxTruncated，
        // 不做任何按声明的分配。
        let layer = TritModelLayer::new(&[vec![1, -1, 0, 1, 0]], &[1]).unwrap();
        let bytes = to_ttx2(&TtxDocument::Model(TritModel::new(vec![layer]).unwrap())).unwrap();
        let outputs_field = HEADER_BYTES + 4 + 4; // count + inputs 之后
        let mut lie = bytes;
        lie[outputs_field..outputs_field + 4].copy_from_slice(&0xFFFF_0000u32.to_le_bytes());
        assert!(matches!(
            from_ttx2(&lie),
            Err(TextureError::TtxTruncated { .. })
        ));
    }

    #[test]
    fn weight_and_bias_corruption_are_caught_by_checksum() {
        let layer = TritModelLayer::new(&[vec![1, -1, 0, 1, -1, 0, 1, 1]], &[1]).unwrap();
        let bytes = to_ttx2(&TtxDocument::Model(TritModel::new(vec![layer]).unwrap())).unwrap();
        let weights_start = HEADER_BYTES + 4 + 8; // count + inputs/outputs 之后
        // 权重字节打翻。
        let mut bad_weight = bytes.clone();
        bad_weight[weights_start] ^= 0x1;
        assert!(matches!(
            from_ttx2(&bad_weight),
            Err(TextureError::TtxChecksum { .. })
        ));
        // 偏置字节打翻也必须被抓到——证明偏置在规范校验流里。
        let mut bad_bias = bytes;
        let bias_start = weights_start + 8usize.div_ceil(TRITS_PER_PACKED_BYTE);
        bad_bias[bias_start] ^= 0x1;
        assert!(matches!(
            from_ttx2(&bad_bias),
            Err(TextureError::TtxChecksum { .. })
        ));
    }

    #[test]
    fn contiguous_sixteen_trit_corruption_is_always_caught() {
        // CRC-16 的强度主张：≤16 连续 trit 的突发错全检（错图样次数 < deg(g)，
        // 不可能被 g 整除）。CRC-3 对此只有概率保证——这是升级的实义。
        let base: Vec<i8> = (0..40).map(|i| [1i8, -1, 0][i % 3]).collect();
        let clean = TritModelLayer::new(std::slice::from_ref(&base), &[0]).unwrap();
        let clean_bytes =
            to_ttx2(&TtxDocument::Model(TritModel::new(vec![clean]).unwrap())).unwrap();
        for start in 0..=40 - 16 {
            // 恰好连续 16 个权重翻转（v → -v），其余不变。
            let mut perturbed = base.clone();
            for value in perturbed.iter_mut().skip(start).take(16) {
                *value = -*value;
            }
            let dirty = TritModelLayer::new(&[perturbed], &[0]).unwrap();
            let dirty_bytes =
                to_ttx2(&TtxDocument::Model(TritModel::new(vec![dirty]).unwrap())).unwrap();
            // 干净容器的头 + 脏载荷 + 干净 CRC：必须被 CRC-16 抓到。
            let mut spliced = clean_bytes.clone();
            let n = clean_bytes.len();
            spliced[HEADER_BYTES..n - CHECKSUM_BYTES]
                .copy_from_slice(&dirty_bytes[HEADER_BYTES..n - CHECKSUM_BYTES]);
            assert!(
                matches!(from_ttx2(&spliced), Err(TextureError::TtxChecksum { .. })),
                "16 连续 trit 突发错从位置 {start} 起漏检"
            );
        }
    }

    #[test]
    fn texture_mip_chain_roundtrips() {
        let fine = encode_rgba(&sample(16, 16), 16, 16).unwrap();
        let coarse = encode_rgba(&sample(8, 8), 8, 8).unwrap();
        let bytes = to_ttx2(&TtxDocument::Texture {
            levels: vec![fine.clone(), coarse.clone()],
        })
        .unwrap();
        assert_eq!(bytes[4], KIND_TEX);
        let TtxDocument::Texture { levels } = from_ttx2(&bytes).unwrap() else {
            panic!("kind 漂移");
        };
        assert_eq!(levels, vec![fine, coarse]);
        // 单级（无 mip）合法：内容与 TTX1 同源。
        let single = to_ttx2(&TtxDocument::Texture {
            levels: vec![encode_rgba(&sample(16, 16), 16, 16).unwrap()],
        })
        .unwrap();
        let TtxDocument::Texture { levels } = from_ttx2(&single).unwrap() else {
            panic!()
        };
        let via_v1 =
            ttx::from_ttx(&ttx::to_ttx(&encode_rgba(&sample(16, 16), 16, 16).unwrap())).unwrap();
        assert_eq!(
            decode_rgba(&levels[0]).unwrap(),
            decode_rgba(&via_v1).unwrap()
        );
    }

    #[test]
    fn mip_chain_rejects_equal_misaligned_and_empty() {
        let fine = encode_rgba(&sample(16, 16), 16, 16).unwrap();
        // 同级（宽高都不减）：拒绝。
        assert!(matches!(
            to_ttx2(&TtxDocument::Texture {
                levels: vec![fine.clone(), fine.clone()],
            }),
            Err(TextureError::TtxMip { level: 1 })
        ));
        // 未对齐尺寸：拒绝。
        let mut odd = fine.clone();
        odd.width = 5;
        assert!(matches!(
            to_ttx2(&TtxDocument::Texture { levels: vec![odd] }),
            Err(TextureError::Shape { width: 5, .. })
        ));
        // 空链：拒绝。
        assert!(matches!(
            to_ttx2(&TtxDocument::Texture { levels: vec![] }),
            Err(TextureError::TtxEmpty)
        ));
        // 容器侧 level_count=0：拒绝。
        let bytes = to_ttx2(&TtxDocument::Texture { levels: vec![fine] }).unwrap();
        let mut empty = bytes;
        empty[HEADER_BYTES..HEADER_BYTES + 4].copy_from_slice(&0u32.to_le_bytes());
        assert!(matches!(from_ttx2(&empty), Err(TextureError::TtxEmpty)));
    }

    #[test]
    fn header_fields_are_validated_before_payload() {
        let bytes = to_ttx2(&TtxDocument::Texture {
            levels: vec![encode_rgba(&sample(8, 8), 8, 8).unwrap()],
        })
        .unwrap();
        // 未知 kind。
        let mut bad_kind = bytes.clone();
        bad_kind[4] = 7;
        assert!(matches!(
            from_ttx2(&bad_kind),
            Err(TextureError::TtxKind(7))
        ));
        // flags=0 是旧 4×10 布局：头部直接报布局错误。
        let mut old_layout = bytes.clone();
        old_layout[5] = 0;
        assert!(matches!(
            from_ttx2(&old_layout),
            Err(TextureError::UnsupportedLayout { rev: 0 })
        ));
        // 未知 flags 位 / reserved 非零仍按前向兼容拒绝。
        let mut bad_flags = bytes.clone();
        bad_flags[5] = FLAG_LAYOUT_8X8 | 0x02;
        assert!(matches!(
            from_ttx2(&bad_flags),
            Err(TextureError::TtxFlags { flags, .. }) if flags == (FLAG_LAYOUT_8X8 | 0x02)
        ));
        let mut bad_reserved = bytes.clone();
        bad_reserved[6] = 1;
        assert!(matches!(
            from_ttx2(&bad_reserved),
            Err(TextureError::TtxFlags { reserved: 1, .. })
        ));
        // 段长谎报。
        let mut lie_len = bytes;
        lie_len[8..12].copy_from_slice(&999u32.to_le_bytes());
        assert!(matches!(
            from_ttx2(&lie_len),
            Err(TextureError::TtxTruncated { .. })
        ));
    }

    #[test]
    fn model_container_bytes_are_pinned() {
        // 金样字节：杀死任何序列化漂移（打包基/digit 映射/头字段/校验流组成/CRC 常数）。
        // 手算核对：权重 [1,-1,0,-1] → digits [2,0,1,0] → 首字节 = 2+9 = 11；
        // bias [1] → digit 2 → 字节 2；段 = 4+4+4+1+1 = 14 字节。
        // CRC-16 尾 = [144, 81, 14, 1]（余数 u32 17715600 小端）——独立 Python
        // GF(3) 长除法交叉验算，非取自本实现。
        let layer = TritModelLayer::new(&[vec![1, -1, 0, -1]], &[1]).unwrap();
        let bytes = to_ttx2(&TtxDocument::Model(TritModel::new(vec![layer]).unwrap())).unwrap();
        let expected = vec![
            b'T',
            b'T',
            b'X',
            b'2',
            KIND_MOD,
            FLAG_LAYOUT_8X8,
            0,
            0,
            14,
            0,
            0,
            0, // 头
            1,
            0,
            0,
            0, // layer_count
            4,
            0,
            0,
            0, // inputs
            1,
            0,
            0,
            0,  // outputs
            11, // weights 基 3 打包（2·3⁰ + 1·3²）
            2,  // bias digit
            144,
            81,
            14,
            1, // GF(3) CRC-16（独立验算冻结）
        ];
        assert_eq!(bytes, expected);
        // 双偏置基 3 位值钉死：bias [1,0] → 2·3⁰ + 1·3¹ = 5（基 2 打包变异体写 4）。
        let two = TritModelLayer::new(&[vec![1, 0, -1, 0], vec![0, 1, 0, -1]], &[1, 0]).unwrap();
        let bytes = to_ttx2(&TtxDocument::Model(TritModel::new(vec![two]).unwrap())).unwrap();
        let weights_end = HEADER_BYTES + 4 + 8 + 8usize.div_ceil(TRITS_PER_PACKED_BYTE);
        assert_eq!(bytes[weights_end], 5);
        // 谎报 layer_count=0：显式拒绝（变异后走校验错误也能被这条抓住）。
        let mut empty = bytes;
        empty[HEADER_BYTES..HEADER_BYTES + 4].copy_from_slice(&0u32.to_le_bytes());
        assert!(matches!(from_ttx2(&empty), Err(TextureError::TtxEmpty)));
    }

    #[test]
    fn absurd_level_declarations_are_rejected_by_block_budget() {
        // mip 级声明 8 亿×20（块数 ×12 > u32::MAX 的回绕区）：level_blocks 的
        // checked 预算必须拒成 Shape，不进入任何按声明的分配/切片。
        let bytes = to_ttx2(&TtxDocument::Texture {
            levels: vec![encode_rgba(&sample(8, 8), 8, 8).unwrap()],
        })
        .unwrap();
        let mut giant = bytes;
        let level_head = HEADER_BYTES + 4; // level_count 之后第一级宽高
        giant[level_head..level_head + 4].copy_from_slice(&800_000_000u32.to_le_bytes());
        giant[level_head + 4..level_head + 8].copy_from_slice(&20u32.to_le_bytes());
        assert!(matches!(
            from_ttx2(&giant),
            Err(TextureError::Shape {
                width: 800_000_000,
                height: 20
            })
        ));
    }

    #[test]
    fn v1_files_are_rejected_with_explicit_magic() {
        let texture = encode_rgba(&sample(8, 8), 8, 8).unwrap();
        let v1 = ttx::to_ttx(&texture);
        assert!(matches!(
            from_ttx2(&v1),
            Err(TextureError::TtxMagic([b'T', b'T', b'X', b'1']))
        ));
    }
}
