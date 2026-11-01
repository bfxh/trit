//! 泄密检测：needle 在运行时拼装，本工具自己的产物里不含有连续样本。
//!
//! 自扫边界（2026-10-28 自审）：needle 以连续字面量编进本工具自己的产物时，
//! `binary` 自检会命中自己——检测样本表的自指误报。所以：
//! - needle 改**运行时逆序片段拼装**（直接分段仍被链接器的字符串相邻布局
//!   拼回明文，实测又命中一次才升级）；
//! - `plausible_secret` 的降噪分支按 **name** 匹配，needle 文本不出现在
//!   比较里（那是第二层自指）。

use std::collections::BTreeMap;

/// 泄密检测 needle 在**运行时**拼装：连续字面量（`AKIA`、`sk-`…）一旦编进
/// 本工具自己的产物，`binary` 自检就会命中自己——检测样本表的自指误报
/// （2026-10-28 自审发现）。
///
/// 表里存**逆序片段**：直接分段（`["AK","IA"]`）仍可能被链接器的字符串
/// 相邻布局拼回明文（实测命中过），逆序后即使相邻也是 `IAAK` 这种乱序，
/// 运行时 `rev()` 还原。
fn secret_patterns() -> Vec<(&'static str, String)> {
    [
        ("private-key", &["BEGIN ", "-----"][..]),
        ("aws-access-key", &["IA", "AK"][..]),
        ("openai-style-key", &["k-", "s"][..]),
        ("github-token", &["p_", "gh"][..]),
        ("slack-token", &["xb-", "xo"][..]),
    ]
    .into_iter()
    .map(|(name, parts)| {
        let mut text = String::new();
        for part in parts.iter().rev() {
            text.push_str(part);
        }
        (name, text)
    })
    .collect()
}

/// 在产物文本里找密钥形态命中（`name` → 出现次数）。
pub(crate) fn scan_strings(bytes: &[u8]) -> BTreeMap<&'static str, usize> {
    let text = String::from_utf8_lossy(bytes);
    let mut hits = BTreeMap::new();
    for (name, needle) in secret_patterns() {
        let count = text.matches(&needle).count();
        if count > 0 && plausible_secret(&text, name, &needle) {
            hits.insert(name, count);
        }
    }
    hits
}

/// 降噪：`sk-` / `AKIA` 这类短前缀在正常文本里也会出现，要求后面跟够长的字母数字串。
/// 分支按 **name** 匹配而不是 needle 文本——needle 字面量出现在比较里
/// 就又成了自指样本（自审教训）。
fn plausible_secret(text: &str, name: &str, needle: &str) -> bool {
    if name == "private-key" {
        return text.contains("PRIVATE KEY-----");
    }
    let min_tail = match name {
        "aws-access-key" => 16,
        "github-token" => 30,
        "openai-style-key" => 20,
        "slack-token" => 20,
        _ => 8,
    };
    let mut rest = text;
    while let Some(position) = rest.find(needle) {
        let tail = &rest[position + needle.len()..];
        let mut used = 0usize;
        let mut run = 0usize;
        for c in tail.chars() {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                run += 1;
                used += c.len_utf8();
            } else {
                break;
            }
        }
        if run >= min_tail {
            return true;
        }
        let step = if used == 0 {
            tail.chars().next().map_or(1, char::len_utf8)
        } else {
            used
        };
        rest = &tail[step..];
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembled_needles_are_exact() {
        let patterns = secret_patterns();
        let table: std::collections::BTreeMap<&str, String> = patterns.into_iter().collect();
        assert_eq!(table["aws-access-key"], "AKIA");
        assert_eq!(table["openai-style-key"], "sk-");
        assert_eq!(table["private-key"], "-----BEGIN ");
        assert_eq!(table["github-token"], "ghp_");
        assert_eq!(table["slack-token"], "xoxb-");
    }

    #[test]
    fn detects_secret_with_tail_and_ignores_plain_word() {
        // 够长的尾随字母数字串才算命中。
        let bytes = b"\0AKIAIOSFODNN7EXAMPLEKEYX\0".to_vec();
        assert_eq!(scan_strings(&bytes)["aws-access-key"], 1);
        // `AKIA` 单独出现、后面没有长尾：降噪过滤。
        let bytes = b"\0the AKIA word\0".to_vec();
        assert!(scan_strings(&bytes).is_empty());
    }

    #[test]
    fn private_key_needs_the_full_header() {
        let bytes = b"\0-----BEGIN OPENSSH PRIVATE KEY-----\0".to_vec();
        assert_eq!(scan_strings(&bytes)["private-key"], 1);
        let bytes = b"\0-----BEGIN SOMETHING ELSE-----\0".to_vec();
        assert!(scan_strings(&bytes).is_empty());
    }
}
