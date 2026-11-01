//! 字节熵：判断"这段数据像不像压缩/加密"的唯一可用单点指标。
//!
//! 只有 0..=8 bit/byte 一个数，没有阈值语义——阈值由事实层决定，
//! 这里保持纯函数，方便用已知分布复算。

/// 香农熵（bit/byte）。空输入定义为 0。
pub fn shannon(bytes: &[u8]) -> f64 {
    if bytes.is_empty() {
        return 0.0;
    }
    let mut counts = [0u64; 256];
    for &byte in bytes {
        counts[byte as usize] += 1;
    }
    let total = bytes.len() as f64;
    let mut entropy = 0.0;
    for count in counts {
        if count > 0 {
            let p = count as f64 / total;
            entropy -= p * p.log2();
        }
    }
    entropy
}

/// ASCII 可打印字节占比（0..=1）。
pub fn printable_ratio(bytes: &[u8]) -> f64 {
    if bytes.is_empty() {
        return 0.0;
    }
    let printable = bytes
        .iter()
        .filter(|b| (0x20..=0x7e).contains(*b) || **b == b'\n' || **b == b'\r' || **b == b'\t')
        .count();
    printable as f64 / bytes.len() as f64
}

#[cfg(test)]
mod tests {
    use super::{printable_ratio, shannon};

    #[test]
    fn uniform_full_byte_range_is_eight_bits() {
        let all: Vec<u8> = (0..=255u8).collect();
        assert!((shannon(&all) - 8.0).abs() < 1e-12);
    }

    #[test]
    fn constant_is_zero() {
        assert_eq!(shannon(&[7u8; 4096]), 0.0);
        assert_eq!(shannon(b""), 0.0);
    }

    #[test]
    fn two_symbols_is_one_bit() {
        let mut data = vec![0u8; 512];
        data.extend(std::iter::repeat_n(1u8, 512));
        assert!((shannon(&data) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn printable_ratio_bounds() {
        assert_eq!(printable_ratio(b"hello\n"), 1.0);
        assert_eq!(printable_ratio(&[0u8; 8]), 0.0);
        assert_eq!(printable_ratio(b""), 0.0);
    }
}
