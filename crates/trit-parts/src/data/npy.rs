//! `.npy`（小端 int32 二维）的读写。
//! WHY 单独放一个文件：头字段全部来自外部二进制，边界要能一条条数清楚 ——
//! 和 PNG/场景加载混着写时，没人会把 `header_len` 当攻击面看（L65）。

use std::path::Path;

/// 读 `.npy`（仅小端整型 2 维数组），返回 `(数据, 形状)`。
pub fn read_npy_i32(path: &Path) -> Result<(Vec<i32>, Vec<usize>), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_npy_i32(&bytes)
}

/// 解析 `.npy` 字节（仅小端整型 2 维）。抽成纯函数，边界才能用字节直接测。
fn parse_npy_i32(bytes: &[u8]) -> Result<(Vec<i32>, Vec<usize>), String> {
    if bytes.len() < 10 || &bytes[0..6] != b"\x93NUMPY" {
        return Err("不是 .npy 文件".into());
    }
    let major = bytes[6];
    let (hlen, start): (usize, usize) = if major == 1 {
        (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10)
    } else {
        // v2.0 的 header_len 是 4 字节，文件至少要有 12 字节才够读它。
        if bytes.len() < 12 {
            return Err("npy v2 头不足 12 字节".into());
        }
        (
            u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
            12,
        )
    };
    let end = start.checked_add(hlen).ok_or("npy 头长度溢出")?;
    if end > bytes.len() {
        return Err(format!("npy 头声称 {hlen} 字节，文件只有 {}", bytes.len()));
    }
    let header = std::str::from_utf8(&bytes[start..end]).map_err(|_| "npy 头非 UTF-8")?;
    let descr = field(header, "descr").ok_or("npy 缺 descr")?;
    if descr != "<i4" && descr != "|i4" {
        return Err(format!("只支持 int32，得到 {descr}"));
    }
    let shape: Vec<usize> = field(header, "shape")
        .ok_or("npy 缺 shape")?
        .trim_matches(|c| c == '(' || c == ')' || c == ',' || c == ' ')
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    let n = shape
        .iter()
        .try_fold(1usize, |a, &b| a.checked_mul(b))
        .ok_or("npy 形状溢出")?;
    let body = &bytes[end..];
    let need = n.checked_mul(4).ok_or("npy 数据长度溢出")?;
    if body.len() < need {
        return Err("npy 数据长度不足".into());
    }
    let data = body[..need]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| i32::from_le_bytes(*c))
        .collect();
    Ok((data, shape))
}

/// 写 int32 二维 `.npy`（v1.0，小端），布局与 `read_npy_i32` 互补。
pub fn write_npy_i32(path: &Path, data: &[i32], w: usize, h: usize) -> Result<(), String> {
    if data.len() != w * h {
        return Err(format!("长度 {} != {w}×{h}", data.len()));
    }
    let mut hdr = format!("{{'descr': '<i4', 'fortran_order': False, 'shape': ({h}, {w}), }}");
    let pad = (64 - (10 + hdr.len() + 1) % 64) % 64;
    hdr.push_str(&" ".repeat(pad));
    hdr.push('\n');
    let mut bytes = Vec::with_capacity(10 + hdr.len() + data.len() * 4);
    bytes.extend_from_slice(b"\x93NUMPY\x01\x00");
    bytes.extend_from_slice(&(hdr.len() as u16).to_le_bytes());
    bytes.extend_from_slice(hdr.as_bytes());
    for v in data {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

fn field<'a>(header: &'a str, key: &str) -> Option<&'a str> {
    let i = header.find(&format!("'{key}'"))? + key.len() + 2;
    let rest = header[i..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    if let Some(r) = rest.strip_prefix('\'') {
        let b = r.find('\'')?;
        Some(&r[..b])
    } else {
        let end = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '(' || c == ')' || c == ',' || c == ' '))
            .unwrap_or(rest.len());
        Some(rest[..end].trim_end())
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn npy_header_is_bounds_checked() {
        // v2 头的 header_len 是 4 字节，文件至少要有 12 字节。
        assert!(parse_npy_i32(b"\x93NUMPY\x02\x00\x00\x00").is_err());
        // header_len 声称 65535 字节，文件里并没有。
        assert!(parse_npy_i32(b"\x93NUMPY\x01\x00\xff\xff").is_err());
        // 一个正常的 2×2 int32。
        let hdr = "{'descr': '<i4', 'fortran_order': False, 'shape': (2, 2), }\n";
        let mut v = b"\x93NUMPY\x01\x00".to_vec();
        v.extend_from_slice(&(hdr.len() as u16).to_le_bytes());
        v.extend_from_slice(hdr.as_bytes());
        v.extend_from_slice(&[1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 4, 0, 0, 0]);
        let (data, shape) = parse_npy_i32(&v).unwrap();
        assert_eq!(shape, vec![2, 2]);
        assert_eq!(data, vec![1, 2, 3, 4]);
    }

    #[test]
    fn npy_i32_roundtrip() {
        let dir = std::env::temp_dir().join("trit_parts_npy_rt");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.npy");
        let data: Vec<i32> = (0..12).map(|i| i * 3 - 7).collect();
        write_npy_i32(&p, &data, 4, 3).unwrap();
        let (back, shape) = read_npy_i32(&p).unwrap();
        assert_eq!(shape, vec![3, 4]);
        assert_eq!(back, data);
    }

    #[test]
    fn npy_roundtrip_shape() {
        // 手工构造一个 2×3 int32 npy（v1.0 头）
        let header = "{'descr': '<i4', 'fortran_order': False, 'shape': (2, 3), }";
        let mut h = header.as_bytes().to_vec();
        while !(10 + h.len()).is_multiple_of(64) {
            h.push(b' ');
        }
        h.push(b'\n');
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"\x93NUMPY\x01\x00");
        bytes.extend_from_slice(&(h.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&h);
        for v in [1i32, 2, 3, 4, 5, 6] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let dir = std::env::temp_dir().join("trit-parts-npy-test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.npy");
        std::fs::write(&p, &bytes).unwrap();
        let (data, shape) = read_npy_i32(&p).unwrap();
        assert_eq!(shape, vec![2, 3]);
        assert_eq!(data, vec![1, 2, 3, 4, 5, 6]);
        std::fs::remove_file(&p).ok();
    }
}
