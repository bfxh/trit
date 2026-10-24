//! TRB2 blob 的第三方解析 + 非规范编码枚举。
//!
//! 这是攻方视角：**不复用 `trit-build` 的代码**，独立实现一遍解包与 CRC-3。
//! 两份实现只要有一边理解错格式，逐 trit 对拍就会红。
//!
//! 已确认的攻击面（`cargo test` 里有判据）：TRB2 的载荷按 5 trit/字节打包，
//! 末字节里没用完的 trit 槽位**不影响解码、也不在 CRC 覆盖范围内**，于是同一张真值表
//! 存在多个字节串（非规范编码）。对内容寻址缓存来说，这意味着「哈希 ≠ 语义」；
//! 对想夹带数据的攻击者来说，每个字节的闲置槽位就是隐写通道。

use anyhow::{Result, bail};

pub const HEADER: usize = 11;

/// 解出来的表（`values` 用 -1/0/1，与 `truth.csv` 的判定列一致）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blob {
    pub algebra: u8,
    pub nvars: u8,
    pub values: Vec<i8>,
    pub payload_len: usize,
    pub bytes: Vec<u8>,
}

fn digit(value: i8) -> u32 {
    (value + 1) as u32
}

fn trit(digit: u32) -> i8 {
    digit as i8 - 1
}

/// GF(3) CRC-3，生成多项式 `x³ + 2x + 1`。
pub fn crc3(data: &[i8]) -> [i8; 3] {
    let mut buf: Vec<u32> = data.iter().map(|value| digit(*value)).collect();
    buf.extend_from_slice(&[0, 0, 0]);
    let total = buf.len();
    for i in 0..total - 3 {
        let lead = buf[i] % 3;
        if lead == 0 {
            continue;
        }
        for (k, coefficient) in [1u32, 0, 2, 1].iter().enumerate() {
            buf[i + k] = (buf[i + k] + 3 - (lead * coefficient) % 3) % 3;
        }
    }
    [
        trit(buf[total - 3]),
        trit(buf[total - 2]),
        trit(buf[total - 1]),
    ]
}

pub fn parse(bytes: &[u8]) -> Result<Blob> {
    if bytes.len() < HEADER + 1 {
        bail!("太短（{} 字节）", bytes.len());
    }
    if &bytes[..4] != b"TRB2" {
        bail!("魔数不是 TRB2");
    }
    if bytes[4] != 2 {
        bail!("不支持的版本 {}", bytes[4]);
    }
    if bytes[5] > 2 {
        bail!("未知语义字节 {}", bytes[5]);
    }
    let nvars = bytes[6];
    let len = u32::from_le_bytes([bytes[7], bytes[8], bytes[9], bytes[10]]) as usize;
    // 表长必须恰好 3^nvars（与 `trit-build` 的 decode 和 C 运行时同一契约）：
    // 查表函数按 nvars 组索引，不符就会越界。第一轮差分 fuzz 就是因为这里缺校验而红。
    if 3usize.checked_pow(nvars as u32) != Some(len) {
        bail!("表长 {len} 与 nvars={nvars} 不符（应为 3^{nvars}）");
    }
    let payload = len + 3;
    let expected = HEADER + payload.div_ceil(5);
    if bytes.len() != expected {
        bail!(
            "长度不符：头声明 {len} trit（应 {expected} 字节），实际 {}",
            bytes.len()
        );
    }
    let mut digits: Vec<i8> = Vec::with_capacity(payload);
    'outer: for byte in &bytes[HEADER..] {
        if *byte >= 243 {
            bail!("非法字节 {byte}：5 trit 只能表示 0..=242");
        }
        let mut value = u32::from(*byte);
        for _ in 0..5 {
            digits.push(trit(value % 3));
            value /= 3;
            if digits.len() == payload {
                break 'outer;
            }
        }
    }
    let values = digits[..len].to_vec();
    if digits[len..] != crc3(&values) {
        bail!("CRC-3 不符");
    }
    Ok(Blob {
        algebra: bytes[5],
        nvars,
        values,
        payload_len: payload,
        bytes: bytes.to_vec(),
    })
}

/// 末字节里没用完的 trit 槽位数（0 表示载荷正好占满）。
pub fn unused_trits(bytes: &[u8]) -> Result<usize> {
    let blob = parse(bytes)?;
    let payload_bytes = blob.bytes.len() - HEADER;
    Ok(payload_bytes * 5 - blob.payload_len)
}

/// 末字节里「已用」的低位槽数（载荷正好占满时为 5）。
fn used_in_last_byte(bytes: &[u8]) -> Result<u32> {
    let blob = parse(bytes)?;
    let payload_bytes = blob.bytes.len() - HEADER;
    Ok((blob.payload_len - 5 * (payload_bytes - 1)) as u32)
}

/// 是否规范编码：末字节里没用到的槽位全为 0。
pub fn is_canonical(bytes: &[u8]) -> Result<bool> {
    let filled = used_in_last_byte(bytes)?;
    let last = *bytes.last().expect("非空") as u32;
    Ok(last == last % 3u32.pow(filled))
}

/// 枚举与输入**逐 trit 等价**、但末字节不同的全部字节串（含输入自身）。
pub fn padding_variants(bytes: &[u8]) -> Result<Vec<Vec<u8>>> {
    let unused = unused_trits(bytes)?;
    if unused == 0 {
        return Ok(vec![bytes.to_vec()]);
    }
    let filled = used_in_last_byte(bytes)?;
    let last = *bytes.last().expect("非空") as u32;
    let base = last % 3u32.pow(filled);
    let mut out = Vec::new();
    for combination in 0..3u32.pow(unused as u32) {
        let mut extra = combination;
        let mut add = 0u32;
        for slot in 0..unused {
            add += (extra % 3) * 3u32.pow(filled + slot as u32);
            extra /= 3;
        }
        let candidate = base + add;
        if candidate >= 243 {
            continue;
        }
        let mut variant = bytes.to_vec();
        let last_index = variant.len() - 1;
        variant[last_index] = candidate as u8;
        out.push(variant);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 由 `trit-build`（main `582b6c6`）对 `a & b`（3 变量）生成的真实产物。
    const FIXTURE: &str = "5452423202000209000000514004";

    fn fixture() -> Vec<u8> {
        (0..FIXTURE.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&FIXTURE[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn parses_a_real_artifact() {
        let blob = parse(&fixture()).unwrap();
        assert_eq!(blob.nvars, 2);
        assert_eq!(blob.algebra, 0);
        assert_eq!(blob.values.len(), 9);
        assert_eq!(blob.payload_len, 12);
        // a & b 在 K3 下的表：F&F=F、F&U=F、T&F=F、U&F=F、U&U=U、T&U=U、F&T=F、U&T=U、T&T=T
        assert_eq!(
            blob.values,
            vec![-1, -1, -1, -1, 0, 0, -1, 0, 1],
            "真值表顺序必须与 truth.csv 一致"
        );
    }

    #[test]
    fn every_padding_variant_decodes_to_the_same_table() {
        let original = fixture();
        let variants = padding_variants(&original).unwrap();
        // 12 个载荷 trit 占 3 字节 = 15 槽 ⇒ 3 个闲置槽 ⇒ 3^3 = 27 个变体（都 < 243）。
        assert_eq!(variants.len(), 27, "闲置 3 trit 应有 27 种写法");
        assert_eq!(unused_trits(&original).unwrap(), 3);
        let canonical = parse(&original).unwrap();
        let mut distinct_bytes = 1usize;
        for variant in &variants {
            let decoded = parse(variant).unwrap();
            assert_eq!(decoded.values, canonical.values, "变体必须解出同一张表");
            if variant != &original {
                distinct_bytes += 1;
            }
        }
        assert_eq!(distinct_bytes, 27, "27 个变体应当是 27 个不同字节串");
        assert!(is_canonical(&original).unwrap(), "真实产物是规范编码");
        let tampered = {
            let mut bytes = original.clone();
            let last = bytes.len() - 1;
            bytes[last] += 9;
            bytes
        };
        assert!(
            !is_canonical(&tampered).unwrap(),
            "加了闲置位就不是规范编码"
        );
        assert_eq!(parse(&tampered).unwrap().values, canonical.values);
    }

    #[test]
    fn rejects_nvars_length_mismatch() {
        let mut bytes = fixture();
        bytes[6] = 3; // 声称 3 变量，但表长仍是 9
        assert!(parse(&bytes).is_err(), "len 与 nvars 不符必须拒绝");
    }

    #[test]
    fn rejects_malformed_blobs() {
        let mut bytes = fixture();
        bytes[0] = b'X';
        assert!(parse(&bytes).is_err());
        let mut bytes = fixture();
        bytes[4] = 9;
        assert!(parse(&bytes).is_err());
        let mut bytes = fixture();
        let last = bytes.len() - 1;
        bytes[last] ^= 0b101;
        assert!(parse(&bytes).is_err(), "翻位必须被 CRC 或字节上限抓住");
    }
}
