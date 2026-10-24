//! TRB2：基 3 打包（5 trit/字节，1.6 bit/trit）+ GF(3) CRC-3。
//!
//! 为什么不再用 M0 的双导轨（2 bit/trit）：TRB2 省 20% 体积，且 CRC-3 只占 3 个 trit。
//! 为什么不在 trit-build 里直接用 `trit-codec`：它是 `publish = false` 的实验层，
//! 发布图门禁禁止实验 crate 互相依赖。这里保留**同构**的精简实现，等 codec 稳定后统一。
//!
//! 格式：`TRB2` | version u8 | algebra u8 | nvars u8 | len u32 LE | packed(ceil((len+3)/5) B)
//! 载荷 = 输出 trit 序列 || CRC-3（3 个 trit），再按 5 trit/字节打包。

use anyhow::{Result, bail};
use trit_core::Algebra;
use trit_core::value::Ternary;

pub const MAGIC: &[u8; 4] = b"TRB2";
pub const VERSION: u8 = 2;
const HEADER: usize = 11;

fn digit(value: Ternary) -> u32 {
    match value {
        Ternary::False => 0,
        Ternary::Unknown => 1,
        Ternary::True => 2,
    }
}

fn trit(digit: u32) -> Ternary {
    match digit % 3 {
        0 => Ternary::False,
        1 => Ternary::Unknown,
        _ => Ternary::True,
    }
}

/// 5 trit → 1 字节（权重：d0 + 3·d1 + 9·d2 + 27·d3 + 81·d4）。
pub fn pack(values: &[Ternary]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len().div_ceil(5));
    for chunk in values.chunks(5) {
        let mut byte = 0u32;
        let mut weight = 1u32;
        for value in chunk {
            byte += digit(*value) * weight;
            weight *= 3;
        }
        debug_assert!(byte < 243);
        out.push(byte as u8);
    }
    out
}

/// 解包；字节 243..=255 不是合法 5-trit 组合，显式报错而不是猜。
pub fn unpack(bytes: &[u8], len: usize) -> Result<Vec<Ternary>> {
    let mut out = Vec::with_capacity(len);
    'outer: for byte in bytes {
        if *byte >= 243 {
            bail!("非法字节 {byte}：5 个 trit 只能表示 0..=242");
        }
        let mut value = u32::from(*byte);
        for _ in 0..5 {
            out.push(trit(value % 3));
            value /= 3;
            if out.len() == len {
                break 'outer;
            }
        }
    }
    if out.len() != len {
        bail!("长度不足：需要 {len} 个 trit，只有 {}", out.len());
    }
    Ok(out)
}

/// GF(3) 上的 CRC-3：生成多项式 `x³ + 2x + 1`（本原）。
pub fn crc3(data: &[Ternary]) -> [Ternary; 3] {
    let mut buf: Vec<u32> = data.iter().map(|v| digit(*v)).collect();
    buf.extend_from_slice(&[0, 0, 0]);
    let n = buf.len();
    for i in 0..n - 3 {
        let lead = buf[i] % 3;
        if lead != 0 {
            for (k, coefficient) in [1u32, 0, 2, 1].iter().enumerate() {
                buf[i + k] = (buf[i + k] + 3 - (lead * coefficient) % 3) % 3;
            }
        }
    }
    [trit(buf[n - 3]), trit(buf[n - 2]), trit(buf[n - 1])]
}

/// 编码：输出 trit || CRC-3，再打包。
pub fn encode(values: &[Ternary], alg: Algebra, nvars: usize) -> Vec<u8> {
    let mut payload = values.to_vec();
    payload.extend_from_slice(&crc3(values));
    let packed = pack(&payload);
    let mut out = Vec::with_capacity(HEADER + packed.len());
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.push(alg as u8);
    out.push(nvars as u8);
    out.extend_from_slice(&(values.len() as u32).to_le_bytes());
    out.extend_from_slice(&packed);
    out
}

/// 解码 + 校验（魔数/版本/语义/长度/CRC 任一不符即报错）。
pub fn decode(blob: &[u8]) -> Result<(Vec<Ternary>, usize, Algebra)> {
    if blob.len() < HEADER + 1 {
        bail!("TRB2 太短（{} 字节）", blob.len());
    }
    if &blob[..4] != MAGIC {
        bail!("魔数不是 TRB2");
    }
    if blob[4] != VERSION {
        bail!("不支持的 TRB2 版本 {}", blob[4]);
    }
    let alg = match blob[5] {
        0 => Algebra::Kleene,
        1 => Algebra::Lukasiewicz,
        2 => Algebra::Bochvar,
        other => bail!("未知语义字节 {other}（0=kleene / 1=lukasiewicz / 2=bochvar）"),
    };
    let nvars = usize::from(blob[6]);
    let len = u32::from_le_bytes([blob[7], blob[8], blob[9], blob[10]]) as usize;
    // 表长必须恰好 3^nvars：查表函数按 nvars 组索引，不符就会越界。
    if 3usize.checked_pow(nvars as u32) != Some(len) {
        bail!("表长 {len} 与 nvars={nvars} 不符（应为 3^{nvars}）");
    }
    let payload_len = len + 3;
    let expected = HEADER + payload_len.div_ceil(5);
    if blob.len() != expected {
        bail!(
            "长度不符：头声明 {len} trit（应 {expected} 字节），实际 {}",
            blob.len()
        );
    }
    let mut all = unpack(&blob[HEADER..], payload_len)?;
    let checksum: Vec<Ternary> = all.split_off(len);
    let values = all;
    if checksum != crc3(&values) {
        bail!("CRC-3 校验失败：载荷可能被篡改或截断");
    }
    Ok((values, nvars, alg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_round_trips_every_byte() {
        for byte in 0..243u8 {
            let mut value = u32::from(byte);
            let mut expected = Vec::new();
            for _ in 0..5 {
                expected.push(trit(value % 3));
                value /= 3;
            }
            assert_eq!(pack(&expected), vec![byte]);
            assert_eq!(unpack(&[byte], 5).unwrap(), expected);
        }
        assert!(unpack(&[243], 5).is_err());
    }

    #[test]
    fn crc_detects_single_point_errors() {
        let data = vec![
            Ternary::True,
            Ternary::False,
            Ternary::Unknown,
            Ternary::True,
        ];
        let code = crc3(&data);
        for index in 0..data.len() {
            for flip in [Ternary::False, Ternary::Unknown, Ternary::True] {
                if data[index] == flip {
                    continue;
                }
                let mut broken = data.clone();
                broken[index] = flip;
                assert_ne!(crc3(&broken), code, "单点错误未被检出：{index}→{flip:?}");
            }
        }
    }

    #[test]
    fn encode_decode_round_trips_and_rejects_tampering() {
        let values: Vec<Ternary> = (0..27)
            .map(|i| match i % 3 {
                0 => Ternary::False,
                1 => Ternary::Unknown,
                _ => Ternary::True,
            })
            .collect();
        let blob = encode(&values, Algebra::Kleene, 3);
        // 27 + 3 = 30 trit / 5 per byte = 6 字节 + 11 字节头。
        assert_eq!(blob.len(), 11 + 6);
        assert_eq!(decode(&blob).unwrap(), (values.clone(), 3, Algebra::Kleene));

        let mut broken = blob.clone();
        let last = broken.len() - 1;
        broken[last] ^= 0b11;
        assert!(decode(&broken).is_err());

        let mut short = blob.clone();
        short.pop();
        assert!(decode(&short).is_err());
    }

    #[test]
    fn decode_then_encode_is_byte_identical_for_any_length() {
        for alg in [Algebra::Kleene, Algebra::Lukasiewicz, Algebra::Bochvar] {
            for nvars in 0..=4u32 {
                let len = 3usize.pow(nvars);
                let values: Vec<Ternary> = (0..len)
                    .map(|i| match i % 3 {
                        0 => Ternary::False,
                        1 => Ternary::Unknown,
                        _ => Ternary::True,
                    })
                    .collect();
                let blob = encode(&values, alg, nvars as usize);
                let (decoded, stored_nvars, decoded_alg) = decode(&blob).unwrap();
                assert_eq!(decoded, values, "nvars={nvars} 解出的 trit 必须相同");
                assert_eq!(stored_nvars, nvars as usize);
                assert_eq!(decoded_alg, alg, "语义字节必须原样带回来");
                assert_eq!(
                    encode(&decoded, decoded_alg, stored_nvars),
                    blob,
                    "nvars={nvars} 重编码必须逐字节相同"
                );
            }
        }
    }
}
