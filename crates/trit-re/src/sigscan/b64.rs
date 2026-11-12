//! base64 串展开（YARA 子集的最后一块编码语义）。
//!
//! 官方语义全部探针实测（yara-python 4.5.4，2026-11-13）：
//! ①**相位算法**：对 p∈{0,1,2}，明文前垫 p 个字节 → 标准 b64 编码 →
//!   去 `=` padding → **头 drop**（p=0 不去、p=1 去 2、p=2 去 3——前缀
//!   字节污染的字符）→ **尾 drop**（(p+len)%3≠0 时去 1——末组不满，
//!   尾字符受后继未知字节影响）→ 空变体丢弃；
//! ②多变体**并集计数**（`#a` 跨变体累计，U1 探针 #a==2 命中）；
//! ③`ascii` 与 base64 组合 = 被忽略（B1/B2）；`base64 base64wide` =
//!   两套变体并集（B7）；`base64` 与 `nocase`/`xor`/`fullword` 官方
//!   **编译拒绝**（P4-P6）→ 解析层对齐拒绝；`wide` 与 base64 组合官方
//!   接受但语义未对拍（B3/B4 双 MISS）→ 记名弃权；
//! ④标准字母表（U2 urlsafe MISS）、大小写敏感（无 nocase）。

use super::{Pattern, TextPattern, Unscannable, find_text};

/// 标准 base64 字母表。
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// 手写 base64 编码（标准字母表，含 `=` padding）。
pub(super) fn encode(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18) as usize & 0x3F]);
        out.push(ALPHABET[(n >> 12) as usize & 0x3F]);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 0x3F]
        } else {
            b'='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 0x3F]
        } else {
            b'='
        });
    }
    out
}

/// wide 展开（UTF-16LE，逐字节补 `\0`）。
fn wide_expand(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(*b);
        out.push(0);
    }
    out
}

/// 相位变体集：`wide_after` = 编码后再 wide（base64wide 修饰）。
pub(super) fn variants(plaintext: &[u8], wide_after: bool) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let len = plaintext.len();
    for p in 0u32..3 {
        let mut data = vec![0u8; p as usize];
        data.extend_from_slice(plaintext);
        let mut enc = encode(&data);
        while enc.last() == Some(&b'=') {
            enc.pop(); // 去 padding（探针：变体不含 `=`）
        }
        let head = if p == 0 { 0 } else { (p + 1) as usize };
        let tail = usize::from(!(p as usize + len).is_multiple_of(3));
        if enc.len() < head + tail {
            continue; // 变体被 drop 光（明文过短）——官方丢弃不生成
        }
        let mut v = enc[head..enc.len() - tail].to_vec();
        if wide_after {
            v = wide_expand(&v);
        }
        if !v.is_empty() {
            out.push(v);
        }
    }
    out
}

/// 多变体并集搜索：逐变体字面匹配，命中按偏移排序去重（官方同一偏移
/// 只记一次），完整性 = 全变体完整。
pub(super) fn find_union(data: &[u8], tps: &[TextPattern]) -> (Vec<u64>, bool) {
    let mut all: Vec<u64> = Vec::new();
    let mut complete = true;
    for tp in tps {
        let (hits, c) = find_text(data, tp);
        all.extend(hits);
        complete &= c;
    }
    all.sort_unstable();
    all.dedup();
    (all, complete)
}

impl super::TextPattern {
    /// 纯字节字面量构造（base64 变体专用——变体是任意字节序列，宽形含
    /// `\0`，不走 &str 路径）。
    pub(super) fn literal(bytes: Vec<u8>) -> Self {
        Self {
            variants: vec![super::TextVariant { bytes, wide: false }],
            nocase: false,
            xor: None,
            fullword: false,
        }
    }
}

impl Pattern {
    /// base64 串入口：`want_std`（base64）/ `want_wide`（base64wide）两套
    /// 相位变体并集（官方 B7 探针：双修饰 = 并集计数）。
    pub fn base64(plaintext: &str, want_std: bool, want_wide: bool) -> Self {
        let mut tps = Vec::new();
        if want_std {
            tps.extend(
                variants(plaintext.as_bytes(), false)
                    .into_iter()
                    .map(TextPattern::literal),
            );
        }
        if want_wide {
            tps.extend(
                variants(plaintext.as_bytes(), true)
                    .into_iter()
                    .map(TextPattern::literal),
            );
        }
        Pattern::Base64(tps)
    }

    /// base64 修饰的组合校验与展开（从 yaraparse.rs 搬入守预算）：
    /// nocase/xor/fullword = 官方编译拒绝（对齐拒绝）；wide 组合官方
    /// 接受但语义未对拍 → 记名弃权；正则串 + base64 = 官方语法错。
    /// `plaintext` = Text 串内容；返回 None = 无 b64 修饰（走常规路径）。
    #[allow(clippy::too_many_arguments)]
    pub fn base64_plan(
        rule_name: &str,
        plaintext: Option<&str>,
        value_is_regex: bool,
        nocase: bool,
        wide: bool,
        xor: bool,
        fullword: bool,
        want_std: bool,
        want_wide: bool,
    ) -> Result<Option<Self>, String> {
        if !(want_std || want_wide) {
            return Ok(None);
        }
        if nocase || xor || fullword {
            return Err(format!(
                "规则 {rule_name}：base64 与 nocase/xor/fullword 组合官方不合法"
            ));
        }
        if value_is_regex {
            return Err(format!(
                "规则 {rule_name}：正则串不支持 base64 修饰（官方语法错）"
            ));
        }
        if wide {
            return Ok(Some(Pattern::Unscannable(Unscannable {
                reason: "base64 与 wide 组合（语义未对拍）".into(),
            })));
        }
        let pt = plaintext.unwrap_or_default();
        Ok(Some(Pattern::base64(pt, want_std, want_wide)))
    }
}

#[cfg(test)]
mod tests {
    use super::{find_union, variants};

    /// 探针回放（yara-python 4.5.4 实测读数，2026-11-13）。
    #[test]
    fn probe_semantics_replay() {
        // "foobar"（6 字节，整组）：3 变体 = 标准全量 + 两相位 drop 后缀
        assert_eq!(
            variants(b"foobar", false),
            vec![
                b"Zm9vYmFy".to_vec(),
                b"Zvb2Jhc".to_vec(),
                b"mb29iYX".to_vec()
            ]
        );
        // "foobarX"（7 字节）：phase0 尾 drop（"Zm9vYmFyW"）；phase2 的
        // 末字符 = 'X' 低 6 位（'Y'）——官方差分 MATCH 11/DIFF 0 实证
        assert_eq!(
            variants(b"foobarX", false),
            vec![
                b"Zm9vYmFyW".to_vec(),
                b"Zvb2Jhcl".to_vec(),
                b"mb29iYXJY".to_vec()
            ]
        );
        // 1 字节：{Q, B}（p2 变体 "B"——官方判别探针 b'B' 命中实证）
        assert_eq!(variants(b"A", false), vec![b"Q".to_vec(), b"B".to_vec()]);
        // 2 字节：{QU, FC, BQ}（p2 变体 "BQ"——官方判别探针 b'BQ' 命中实证）
        assert_eq!(
            variants(b"AB", false),
            vec![b"QU".to_vec(), b"FC".to_vec(), b"BQ".to_vec()]
        );
        // base64wide：编码后逐字节 \0（\x00 避开八进制转义形）
        assert_eq!(
            variants(b"foobar", true),
            vec![
                b"Z\x00m\x009\x00v\x00Y\x00m\x00F\x00y\x00".to_vec(),
                b"Z\x00v\x00b\x002\x00J\x00h\x00c\x00".to_vec(),
                b"m\x00b\x002\x009\x00i\x00Y\x00X\x00".to_vec()
            ]
        );
    }

    #[test]
    fn union_counts_across_variants() {
        // U1 探针：标准 + phase1 编码同文件 → 并集 2 命中
        let tps: Vec<_> = variants(b"foobar", false)
            .into_iter()
            .map(super::TextPattern::literal)
            .collect();
        let data = b"Zm9vYmFy|Zvb2Jhc";
        let (hits, complete) = find_union(data, &tps);
        assert_eq!(hits, vec![0, 9]);
        assert!(complete);
    }
}
