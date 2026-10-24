//! **TTX2**——多载荷容器：kind=TEX（纹理 + mip 链）/ kind=MOD（三值模型）。
//!
//! 布局与竞争力对比见 `docs/TTX.md`；校验哲学与 TTX1 相同：GF(3) CRC-3 按
//! **内容自己的基数**算，逐段验证，任何不一致都是结构化错误。
//!
//! 模型域（三值层/双轨打包/偏置打包）在私有 `model` 子模块，类型经本模块再导出。

mod model;

pub use model::{TRITS_PER_WORD, TritModel, TritModelLayer};

use trit_core::value::Ternary;

use crate::ttx::{block_trits_v2, gf3_remainder};
use crate::{BLOCK_HEIGHT, BLOCK_WIDTH, TRIT_BLOCK_BYTES, TextureError, TritTexture};
use model::{TRITS_PER_BIAS_BYTE, model_trits, pack_bias, unpack_bias};

/// 容器魔数。
pub const MAGIC: [u8; 4] = *b"TTX2";
/// 定头字节数（magic + kind + flags + reserved + section_len）。
pub const HEADER_BYTES: usize = 12;
/// 校验尾字节数（3 trit 打进 1 字节）。
pub const CHECKSUM_BYTES: usize = 1;
/// kind：纹理 + mip 链。
pub const KIND_TEX: u8 = 0;
/// kind：三值模型（双轨权重）。
pub const KIND_MOD: u8 = 1;

/// 一份 TTX2 文档。
#[derive(Debug, Clone, PartialEq)]
pub enum TtxDocument {
    /// 纹理 + mip 链（≥1 级；每级宽高不增且至少一维严格递减）。
    Texture {
        /// 从细到粗的各级纹理。
        levels: Vec<TritTexture>,
    },
    /// 三值模型（线性层链，双轨 GPU-ready 权重）。
    Model(TritModel),
}

/// TEX 的规范 trit 流：各级全部索引 trit 依序拼接。
fn texture_trits(levels: &[TritTexture]) -> Vec<Ternary> {
    let mut trits = Vec::new();
    for level in levels {
        for block in &level.blocks {
            trits.extend_from_slice(&block_trits_v2(block));
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
        .filter(|blocks| blocks * TRIT_BLOCK_BYTES <= u32::MAX as usize)
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
        let words = layer.words_per_row();
        section.extend_from_slice(&(layer.inputs as u32).to_le_bytes());
        section.extend_from_slice(&(layer.outputs as u32).to_le_bytes());
        section.extend_from_slice(&(words as u32).to_le_bytes());
        for word in &layer.pos {
            section.extend_from_slice(&word.to_le_bytes());
        }
        for word in &layer.neg {
            section.extend_from_slice(&word.to_le_bytes());
        }
        section.extend_from_slice(&pack_bias(&layer.bias));
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
    out.push(0);
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(section.len() as u32).to_le_bytes());
    out.extend_from_slice(&section);
    out.push(gf3_remainder(&checksum_trits)[0]);
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
        if cursor + 8 > section.len() {
            return Err(truncated());
        }
        let width = u32::from_le_bytes(section[cursor..cursor + 4].try_into().unwrap()) as usize;
        let height =
            u32::from_le_bytes(section[cursor + 4..cursor + 8].try_into().unwrap()) as usize;
        cursor += 8;
        let blocks = level_blocks(width, height).ok_or(TextureError::Shape { width, height })?;
        let payload = blocks * TRIT_BLOCK_BYTES;
        if cursor + payload > section.len() {
            return Err(truncated());
        }
        let mut block_list = Vec::with_capacity(blocks);
        for index in 0..blocks {
            let start = cursor + index * TRIT_BLOCK_BYTES;
            let mut block = [0u8; TRIT_BLOCK_BYTES];
            block.copy_from_slice(&section[start..start + TRIT_BLOCK_BYTES]);
            block_list.push(block);
        }
        cursor += payload;
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
    let mut cursor = 0usize;
    let take_u32 = |cursor: &mut usize, section: &[u8]| -> Option<u32> {
        if *cursor + 4 > section.len() {
            return None;
        }
        let value = u32::from_le_bytes(section[*cursor..*cursor + 4].try_into().unwrap());
        *cursor += 4;
        Some(value)
    };
    let layer_count = take_u32(&mut cursor, section).ok_or(TextureError::TtxTruncated {
        actual: section.len(),
    })?;
    if layer_count == 0 {
        return Err(TextureError::TtxEmpty);
    }
    let mut layers = Vec::new();
    for layer in 0..layer_count as usize {
        let inputs = take_u32(&mut cursor, section).ok_or(TextureError::TtxTruncated {
            actual: section.len(),
        })? as usize;
        let outputs = take_u32(&mut cursor, section).ok_or(TextureError::TtxTruncated {
            actual: section.len(),
        })? as usize;
        let declared_words = take_u32(&mut cursor, section).ok_or(TextureError::TtxTruncated {
            actual: section.len(),
        })? as usize;
        if inputs == 0 || outputs == 0 {
            return Err(TextureError::TtxModelShape {
                layer,
                message: "inputs/outputs 必须非零".to_string(),
            });
        }
        if declared_words != inputs.div_ceil(TRITS_PER_WORD) {
            return Err(TextureError::TtxModelShape {
                layer,
                message: format!(
                    "字数应为 ceil(inputs/32) = {}",
                    inputs.div_ceil(TRITS_PER_WORD)
                ),
            });
        }
        let words = outputs
            .checked_mul(declared_words)
            .ok_or(TextureError::TtxModelShape {
                layer,
                message: "outputs × words 溢出".to_string(),
            })?;
        let rail_bytes = words.checked_mul(4).ok_or(TextureError::TtxModelShape {
            layer,
            message: "字节数溢出".to_string(),
        })?;
        if cursor + rail_bytes * 2 > section.len() {
            return Err(TextureError::TtxTruncated {
                actual: section.len(),
            });
        }
        let read_rail = |section: &[u8], cursor: &mut usize| -> Vec<u32> {
            let rail = section[*cursor..*cursor + rail_bytes]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|chunk| u32::from_le_bytes(*chunk))
                .collect();
            *cursor += rail_bytes;
            rail
        };
        let pos = read_rail(section, &mut cursor);
        let neg = read_rail(section, &mut cursor);
        for (word, (pos, neg)) in pos.iter().zip(&neg).enumerate() {
            if pos & neg != 0 {
                return Err(TextureError::TtxRailOverlap { layer, word });
            }
        }
        let bias_bytes = outputs.div_ceil(TRITS_PER_BIAS_BYTE);
        if cursor + bias_bytes > section.len() {
            return Err(TextureError::TtxTruncated {
                actual: section.len(),
            });
        }
        let bias = unpack_bias(&section[cursor..cursor + bias_bytes], outputs);
        cursor += bias_bytes;
        layers.push(TritModelLayer {
            inputs,
            outputs,
            pos,
            neg,
            bias,
        });
    }
    Ok(TritModel { layers })
}

/// 从 TTX2 容器加载：逐段校验（魔数/kind/flags/段长/形状/双轨不变式/校验和）。
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
    if flags != 0 || reserved != 0 {
        return Err(TextureError::TtxFlags { flags, reserved });
    }
    let section_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    if bytes.len() != HEADER_BYTES + section_len + CHECKSUM_BYTES {
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
    let stored = bytes[HEADER_BYTES + section_len];
    let expected = gf3_remainder(&trits)[0];
    if stored != expected {
        return Err(TextureError::TtxChecksum { stored, expected });
    }
    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{decode_rgba, encode_rgba};

    fn sample(width: usize, height: usize) -> Vec<u8> {
        (0..width * height * 4).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn model_container_rejects_word_count_lie_and_rail_overlap() {
        // inputs=33 → 真 W=2、floor(33/32)=1：谎报 1 字只有对账（div_ceil）才抓得到。
        let mut row = vec![0i8; 33];
        row[0] = 1;
        row[1] = -1;
        let layer = TritModelLayer::new(&[row], &[1]).unwrap();
        let bytes = to_ttx2(&TtxDocument::Model(TritModel::new(vec![layer]).unwrap())).unwrap();
        let words_field = HEADER_BYTES + 4 + 8; // 层头：inputs/outputs 之后
        assert_eq!(
            u32::from_le_bytes(bytes[words_field..words_field + 4].try_into().unwrap()),
            2,
            "33 输入应为 2 字"
        );
        let mut lie = bytes.clone();
        lie[words_field..words_field + 4].copy_from_slice(&1u32.to_le_bytes());
        assert!(matches!(
            from_ttx2(&lie),
            Err(TextureError::TtxModelShape { layer: 0, .. })
        ));
        // 轨重叠（neg 添上与 pos 同位的 bit0）：解析先于校验和拒绝。
        let mut overlap = bytes;
        let neg_start = HEADER_BYTES + 4 + 12 + 2 * 4; // 头 + count + 层头 + pos 2 字
        overlap[neg_start] |= 0x1;
        assert!(matches!(
            from_ttx2(&overlap),
            Err(TextureError::TtxRailOverlap { layer: 0, word: 0 })
        ));
    }

    #[test]
    fn weight_and_bias_corruption_are_caught_by_checksum() {
        let layer = TritModelLayer::new(&[vec![1, -1, 0, 1, -1, 0, 1, 1]], &[1]).unwrap();
        let bytes = to_ttx2(&TtxDocument::Model(TritModel::new(vec![layer]).unwrap())).unwrap();
        // 权重位翻转：pos 首字最低位。
        let mut bad_weight = bytes.clone();
        let pos_start = HEADER_BYTES + 4 + 12;
        bad_weight[pos_start] ^= 0x1;
        assert!(matches!(
            from_ttx2(&bad_weight),
            Err(TextureError::TtxChecksum { .. })
        ));
        // 偏置字节打翻也必须被抓到——证明偏置在规范校验流里。
        let mut bad_bias = bytes;
        // pos 1 字 + neg 1 字（outputs=1，W=1）之后是 bias。
        let bias_start = pos_start + 4 + 4;
        bad_bias[bias_start] ^= 0x1;
        assert!(matches!(
            from_ttx2(&bad_bias),
            Err(TextureError::TtxChecksum { .. })
        ));
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
        // 单级（无 mip）合法：与多级版同源实现，解码结果必须一致。
        let source = encode_rgba(&sample(16, 16), 16, 16).unwrap();
        let single = to_ttx2(&TtxDocument::Texture {
            levels: vec![source.clone()],
        })
        .unwrap();
        let TtxDocument::Texture { levels } = from_ttx2(&single).unwrap() else {
            panic!()
        };
        assert_eq!(levels, vec![source.clone()]);
        assert_eq!(
            decode_rgba(&levels[0]).unwrap(),
            decode_rgba(&source).unwrap()
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
        // 非零 flags / reserved。
        let mut bad_flags = bytes.clone();
        bad_flags[5] = 1;
        assert!(matches!(
            from_ttx2(&bad_flags),
            Err(TextureError::TtxFlags { flags: 1, .. })
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
        // 金样字节：杀死任何序列化漂移（位序/打包常数/头字段/校验流组成）。
        // 手算核对：pos=[0b0001]（col0 的 +1）、neg=[0b1010]（col1/col3 的 -1）、
        // bias digit = 1+1 = 2；段 = 4+12+4+4+1 = 25 字节。
        let layer = TritModelLayer::new(&[vec![1, -1, 0, -1]], &[1]).unwrap();
        let bytes = to_ttx2(&TtxDocument::Model(TritModel::new(vec![layer]).unwrap())).unwrap();
        let expected = vec![
            b'T', b'T', b'X', b'2', KIND_MOD, 0, 0, 0, 25, 0, 0, 0, // 头
            1, 0, 0, 0, // layer_count
            4, 0, 0, 0, // inputs
            1, 0, 0, 0, // outputs
            1, 0, 0, 0, // words_per_row
            0b0001, 0, 0, 0, // pos
            0b1010, 0, 0, 0,  // neg
            2,  // bias digit
            10, // GF(3) CRC-3（独立 Python 实现交叉验算过）
        ];
        assert_eq!(bytes, expected);
        // 双偏置打包基 3 的位值钉死：bias [1,0] → byte = 2·3⁰ + 1·3¹ = 5
        //（基 2 打包变异体会写出 4）。
        let two = TritModelLayer::new(&[vec![1, 0, -1, 0], vec![0, 1, 0, -1]], &[1, 0]).unwrap();
        let bytes = to_ttx2(&TtxDocument::Model(TritModel::new(vec![two]).unwrap())).unwrap();
        let bias_byte = HEADER_BYTES + 4 + 12 + 4 * 2 + 4 * 2; // 层头 + pos/neg 各 2 字
        assert_eq!(bytes[bias_byte], 5);
        // 谎报 layer_count=0：显式拒绝（变异后走截断错误也能被这条抓住）。
        let mut empty = bytes;
        empty[HEADER_BYTES..HEADER_BYTES + 4].copy_from_slice(&0u32.to_le_bytes());
        assert!(matches!(from_ttx2(&empty), Err(TextureError::TtxEmpty)));
    }

    #[test]
    fn v1_files_are_rejected_with_explicit_magic() {
        // v1 写器在当前布局下已拒绝写出（TTX1 冻结在 4×10），所以这里直接构造
        // 一段合法的 TTX1 头字节：v2 读器必须按魔数显式拒绝，而不是当 TEX 去解。
        let mut v1 = Vec::new();
        v1.extend_from_slice(b"TTX1");
        v1.extend_from_slice(&4u32.to_le_bytes());
        v1.extend_from_slice(&10u32.to_le_bytes());
        v1.extend_from_slice(&1u32.to_le_bytes());
        v1.extend_from_slice(&[0u8; 12]);
        v1.push(0);
        assert!(matches!(
            from_ttx2(&v1),
            Err(TextureError::TtxMagic([b'T', b'T', b'X', b'1']))
        ));
    }
}
