//! WSP3 的 PackBits 字节级 RLE。

use crate::error::{Error, Result};

/// PackBits 编码：literal 1–128 字节（header 0..=127）与重复 3–130 次（header `0x80|(n-3)` + 1 字节）。
#[must_use]
pub fn rle_encode(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut index = 0;
    while index < input.len() {
        let mut run = 1;
        while index + run < input.len() && input[index + run] == input[index] && run < 130 {
            run += 1;
        }
        if run >= 3 {
            out.push(0x80 | u8::try_from(run - 3).expect("run <= 130"));
            out.push(input[index]);
            index += run;
            continue;
        }
        let start = index;
        let mut end = index;
        while end < input.len() && end - start < 128 {
            if end + 2 < input.len() && input[end] == input[end + 1] && input[end] == input[end + 2]
            {
                break;
            }
            end += 1;
        }
        out.push(u8::try_from(end - start - 1).expect("literal <= 128"));
        out.extend_from_slice(&input[start..end]);
        index = end;
    }
    out
}

/// PackBits 解码；解压长度必须等于 `expected`，超出/不足都报错。
pub fn rle_decode(input: &[u8], expected: usize) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(expected);
    let mut index = 0;
    while index < input.len() {
        let header = input[index];
        index += 1;
        if header & 0x80 == 0 {
            let len = usize::from(header) + 1;
            let end = index
                .checked_add(len)
                .ok_or_else(|| Error::Wire("WSP3 literal 长度溢出".into()))?;
            if end > input.len() {
                return Err(Error::Wire("WSP3 literal 截断".into()));
            }
            out.extend_from_slice(&input[index..end]);
            index = end;
        } else {
            let len = usize::from(header & 0x7F) + 3;
            let byte = *input
                .get(index)
                .ok_or_else(|| Error::Wire("WSP3 重复字节缺失".into()))?;
            index += 1;
            out.extend(std::iter::repeat_n(byte, len));
        }
        if out.len() > expected {
            return Err(Error::Wire(format!("WSP3 解包超过原始长度 {expected}")));
        }
    }
    if out.len() != expected {
        return Err(Error::Wire(format!(
            "WSP3 解包长度 {} 与原始长度 {expected} 不符",
            out.len()
        )));
    }
    Ok(out)
}
