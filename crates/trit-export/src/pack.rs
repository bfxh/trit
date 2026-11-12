//! 5 trits/byte 打包（与 `trit-codec::pack` 的线格式一致；本 crate 自带一份，
//! 避免实验层之间的运行时依赖，dev-dependency 仅用于对拍测试）。

use trit_core::Ternary;

use crate::Error;

/// 5 trits/byte（低位在前，`byte = Σ trit[i]·3^i`），尾部 padding 为 False。
pub(crate) fn pack(values: &[Ternary]) -> Vec<u8> {
    let mut out = vec![0u8; values.len().div_ceil(5)];
    for (index, value) in values.iter().enumerate() {
        let code = match value {
            Ternary::False => 0u8,
            Ternary::Unknown => 1,
            Ternary::True => 2,
        };
        out[index / 5] += code * 3u8.pow(u32::try_from(index % 5).expect("index % 5 < 5"));
    }
    out
}

/// 严格解码：长度必须 `ceil(len/5)`、每个字节 ≤242、padding 必须规范。
pub(crate) fn unpack_canonical(bytes: &[u8], len: usize) -> Result<Vec<Ternary>, Error> {
    let expected = len.div_ceil(5);
    if bytes.len() != expected {
        return Err(Error::PackedLen {
            expected,
            got: bytes.len(),
        });
    }
    if let Some(&byte) = bytes.iter().find(|&&byte| byte > 242) {
        return Err(Error::InvalidByte(byte));
    }
    let mut out = Vec::with_capacity(len);
    for index in 0..len {
        let power = 3u8.pow(u32::try_from(index % 5).expect("index % 5 < 5"));
        let code = (bytes[index / 5] / power) % 3;
        out.push(match code {
            0 => Ternary::False,
            1 => Ternary::Unknown,
            _ => Ternary::True,
        });
    }
    if pack(&out) != bytes {
        return Err(Error::Padding);
    }
    Ok(out)
}
