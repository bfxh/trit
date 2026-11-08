//! TRB2：基 3 打包（5 trit/字节，1.6 bit/trit）+ GF(3) CRC-3。
//!
//! 为什么不再用 M0 的双导轨（2 bit/trit）：TRB2 省 20% 体积，且 CRC-3 只占 3 个 trit。
//! 为什么不在 trit-build 里直接用 `trit-codec`：它是 `publish = false` 的实验层，
//! 发布图门禁禁止实验 crate 互相依赖。这里保留**同构**的精简实现，等 codec 稳定后统一。
//!
//! v2：`TRB2` | version=2 | algebra u8 | nvars u8 | len u32 LE | packed(ceil((len+3)/5) B)
//! v3：`TRB2` | version=3 | algebra u8 | nvars u8 | len u32 LE | names_hash u64 LE
//!     | nvars × (name_len u8 | UTF-8 名字) | packed(ceil((len+3)/5) B)
//!
//! 载荷 = 输出 trit 序列 || CRC-3（3 个 trit），再按 5 trit/字节打包。
//! v3 把变量名存进 blob，`--decode` 才能重建原列名；v2 仍可解码（列名回退 `v0..vN`）。
//! `names_hash` 是名字段的 FNV-1a 64（CRC-3 只覆盖 trit 载荷，管不到名字）。

use anyhow::{Result, bail};
use trit_core::Algebra;
use trit_core::value::Ternary;

pub const MAGIC: &[u8; 4] = b"TRB2";
pub const VERSION: u8 = 3;
const HEADER_V2: usize = 11;
const HEADER_V3: usize = 19;

/// 存进 blob 的名字必须能安全写回 CSV 表头与 C 注释：
/// 字母/数字/`_`/`$`（Verilog 端口允许 `$`），长度 1..=255 字节。
pub fn is_safe_name(name: &str) -> bool {
    if name.is_empty() || name.len() > u8::MAX as usize {
        return false;
    }
    let mut chars = name.chars();
    let first = chars.next().expect("非空");
    (first.is_alphabetic() || first == '_' || first == '$')
        && chars.all(|ch| ch.is_alphanumeric() || ch == '_' || ch == '$')
}

fn fnv64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

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

/// 编码 v2（无名）：只给回归测试用；新构建用 [`encode_named`]。
#[cfg(test)]
pub fn encode(values: &[Ternary], alg: Algebra, nvars: usize) -> Vec<u8> {
    encode_inner(values, alg, nvars, None)
}

/// 编码 v3：把变量名写进 blob，`decode` 可原样取回。
pub fn encode_named(values: &[Ternary], alg: Algebra, names: &[String]) -> Result<Vec<u8>> {
    if names.len() > u8::MAX as usize {
        bail!("变量数 {} 超过 TRB2 v3 的名字计数上限 255", names.len());
    }
    for name in names {
        if !is_safe_name(name) {
            bail!("变量名 {name:?} 不能存入 TRB2（只允许字母/数字/_/$，长度 1..=255 字节）");
        }
    }
    Ok(encode_inner(values, alg, names.len(), Some(names)))
}

fn encode_inner(
    values: &[Ternary],
    alg: Algebra,
    nvars: usize,
    names: Option<&[String]>,
) -> Vec<u8> {
    let mut payload = values.to_vec();
    payload.extend_from_slice(&crc3(values));
    let packed = pack(&payload);
    let mut out = Vec::with_capacity(HEADER_V3 + packed.len());
    out.extend_from_slice(MAGIC);
    match names {
        Some(names) => {
            out.push(VERSION);
            out.push(alg as u8);
            out.push(nvars as u8);
            out.extend_from_slice(&(values.len() as u32).to_le_bytes());
            let mut section = Vec::new();
            for name in names {
                section.push(name.len() as u8);
                section.extend_from_slice(name.as_bytes());
            }
            out.extend_from_slice(&fnv64(&section).to_le_bytes());
            out.extend_from_slice(&section);
        }
        None => {
            out.push(2);
            out.push(alg as u8);
            out.push(nvars as u8);
            out.extend_from_slice(&(values.len() as u32).to_le_bytes());
        }
    }
    out.extend_from_slice(&packed);
    out
}

/// 解码 + 校验（魔数/版本/语义/长度/名字/CRC 任一不符即报错）。
///
/// v2 没有名字段，返回 `v0..vN` 作为列名；v3 返回存进去的原名。
pub fn decode(blob: &[u8]) -> Result<(Vec<Ternary>, Vec<String>, Algebra)> {
    if blob.len() < HEADER_V2 + 1 {
        bail!("TRB2 太短（{} 字节）", blob.len());
    }
    if &blob[..4] != MAGIC {
        bail!("魔数不是 TRB2");
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
    let (offset, names) = match blob[4] {
        2 => (
            HEADER_V2,
            (0..nvars).map(|index| format!("v{index}")).collect(),
        ),
        VERSION => {
            if blob.len() < HEADER_V3 {
                bail!("TRB2 v3 太短（{} 字节）", blob.len());
            }
            let stored_hash = u64::from_le_bytes([
                blob[11], blob[12], blob[13], blob[14], blob[15], blob[16], blob[17], blob[18],
            ]);
            let mut offset = HEADER_V3;
            let mut names = Vec::with_capacity(nvars);
            for _ in 0..nvars {
                let Some(&name_len) = blob.get(offset) else {
                    bail!("名字段被截断：声明 {nvars} 个变量，只读到 {}", names.len());
                };
                let name_len = usize::from(name_len);
                offset += 1;
                if name_len == 0 {
                    bail!("名字段含空名字");
                }
                let Some(bytes) = blob.get(offset..offset + name_len) else {
                    bail!("名字段被截断：声明 {nvars} 个变量，只读到 {}", names.len());
                };
                let Ok(name) = std::str::from_utf8(bytes) else {
                    bail!("第 {} 个变量名不是合法 UTF-8", names.len());
                };
                if !is_safe_name(name) {
                    bail!("第 {} 个变量名 {name:?} 含不安全字符", names.len());
                }
                names.push(name.to_string());
                offset += name_len;
            }
            if fnv64(&blob[HEADER_V3..offset]) != stored_hash {
                bail!("变量名段校验失败：names_hash 不符");
            }
            (offset, names)
        }
        other => bail!("不支持的 TRB2 版本 {other}（支持 2 和 {VERSION}）"),
    };
    let payload_len = len + 3;
    let expected = offset + payload_len.div_ceil(5);
    if blob.len() != expected {
        bail!(
            "长度不符：头声明 {len} trit（应 {expected} 字节），实际 {}",
            blob.len()
        );
    }
    // 规范形：末字节里没用到的 trit 槽必须为 0。否则同一张表有多个字节串，
    // 「内容哈希 = 身份」就不成立（red-team 的 encode 检查实测出 27 种写法）。
    let payload_bytes = expected - offset;
    let used_in_last = payload_len - 5 * (payload_bytes - 1);
    if u32::from(*blob.last().expect("非空")) >= 3u32.pow(used_in_last as u32) {
        bail!("非规范编码：末字节闲置的 trit 槽非零（同一张表只允许一种字节串）");
    }
    let mut all = unpack(&blob[offset..], payload_len)?;
    let checksum: Vec<Ternary> = all.split_off(len);
    let values = all;
    if checksum != crc3(&values) {
        bail!("CRC-3 校验失败：载荷可能被篡改或截断");
    }
    Ok((values, names, alg))
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
        let names = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let blob = encode_named(&values, Algebra::Kleene, &names).unwrap();
        // 19 字节 v3 头 + 名字段 3×(1+1) + 30 trit / 5 = 6 字节载荷。
        assert_eq!(blob.len(), 19 + 6 + 6);
        assert_eq!(
            decode(&blob).unwrap(),
            (values.clone(), names.clone(), Algebra::Kleene)
        );

        let mut broken = blob.clone();
        let last = broken.len() - 1;
        broken[last] ^= 0b11;
        assert!(decode(&broken).is_err());

        let mut short = blob.clone();
        short.pop();
        assert!(decode(&short).is_err());

        // 名字段被改但长度不变：CRC 管不到名字，必须由 names_hash 拦下。
        let mut renamed = blob.clone();
        renamed[19 + 1] = b'z';
        let error = decode(&renamed).unwrap_err().to_string();
        assert!(error.contains("names_hash"), "{error}");
    }

    #[test]
    fn v3_names_survive_unicode_and_reject_unsafe_bytes() {
        let values = vec![Ternary::Unknown; 3];
        let names = vec!["变量_1".to_string()];
        let blob = encode_named(&values, Algebra::Bochvar, &names).unwrap();
        assert_eq!(decode(&blob).unwrap(), (values, names, Algebra::Bochvar));

        let values = vec![Ternary::Unknown; 3];
        for bad in ["", "a,b", "a\nb", "a*/b", "a-b"] {
            assert!(
                encode_named(&values, Algebra::Kleene, &[bad.to_string()]).is_err(),
                "{bad:?} 不该能写进 blob"
            );
        }
    }

    #[test]
    fn v2_blobs_still_decode_with_synthetic_names() {
        let values = vec![Ternary::True, Ternary::False, Ternary::Unknown];
        let blob = encode(&values, Algebra::Kleene, 1);
        let (decoded, names, alg) = decode(&blob).unwrap();
        assert_eq!(decoded, values);
        assert_eq!(names, vec!["v0".to_string()]);
        assert_eq!(alg, Algebra::Kleene);
        assert_eq!(encode(&decoded, alg, names.len()), blob);
    }

    #[test]
    fn rejects_non_canonical_padding() {
        let values: Vec<Ternary> = (0..9)
            .map(|i| match i % 3 {
                0 => Ternary::False,
                1 => Ternary::Unknown,
                _ => Ternary::True,
            })
            .collect();
        let names = vec!["a".to_string(), "b".to_string()];
        let blob = encode_named(&values, Algebra::Kleene, &names).unwrap();
        // 9 + 3 = 12 trit 占 3 字节 = 15 槽 ⇒ 末字节有 3 个闲置槽（权重 9/27/81）。
        let mut padded = blob.clone();
        let last = padded.len() - 1;
        padded[last] += 9;
        assert!(u32::from(padded[last]) < 243, "仍是可表示字节");
        assert!(
            decode(&padded).is_err(),
            "闲置槽非零的写法必须拒绝（否则同一张表有多个字节串）"
        );
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
                let names: Vec<String> = (0..nvars).map(|index| format!("x{index}")).collect();
                let blob = encode_named(&values, alg, &names).unwrap();
                let (decoded, decoded_names, decoded_alg) = decode(&blob).unwrap();
                assert_eq!(decoded, values, "nvars={nvars} 解出的 trit 必须相同");
                assert_eq!(decoded_names, names, "nvars={nvars} 名字必须原样带回");
                assert_eq!(decoded_alg, alg, "语义字节必须原样带回来");
                assert_eq!(
                    encode_named(&decoded, decoded_alg, &decoded_names).unwrap(),
                    blob,
                    "nvars={nvars} 重编码必须逐字节相同"
                );
            }
        }
    }
}
