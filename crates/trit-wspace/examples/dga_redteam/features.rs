//! DGA 域名的字符 n-gram 三值特征（存在性编码，输入本身三值）。

/// 特征维度：域名短（平均 ~25 字符），2/3-gram 域内数量 ~50，2048 桶够用。
pub const FEAT_DIM: usize = 2048;

pub(super) fn fnv(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// 域名 2/3-gram 哈希：出现=+1（小写化，保留数字/连字符——DGA 的关键信号）。
pub fn featurize(domain: &str) -> Vec<f32> {
    let lower: Vec<char> = domain.to_lowercase().chars().collect();
    let mut feats = vec![0.0f32; FEAT_DIM];
    for width in [2usize, 3] {
        if lower.len() < width {
            continue;
        }
        for start in 0..=(lower.len() - width) {
            let mut bytes = Vec::with_capacity(width * 2);
            for ch in &lower[start..start + width] {
                let mut buf = [0u8; 4];
                bytes.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            }
            let slot = (fnv(&bytes) % FEAT_DIM as u64) as usize;
            feats[slot] = 1.0;
        }
    }
    feats
}
