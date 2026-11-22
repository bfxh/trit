//! 多进制数字识别：二进制 / 八进制 / 十进制 / 十六进制。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadixKind {
    Binary,
    Octal,
    Decimal,
    Hex,
    Mixed,
    None,
}

impl RadixKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RadixKind::Binary => "binary",
            RadixKind::Octal => "octal",
            RadixKind::Decimal => "decimal",
            RadixKind::Hex => "hex",
            RadixKind::Mixed => "mixed",
            RadixKind::None => "none",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RadixCounts {
    pub binary: usize,
    pub octal: usize,
    pub decimal: usize,
    pub hex: usize,
}

pub fn counts(text: &str) -> RadixCounts {
    let mut out = RadixCounts::default();
    let mut token = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            token.push(ch);
        } else {
            flush(&mut token, &mut out);
        }
    }
    flush(&mut token, &mut out);
    out
}

fn flush(token: &mut String, out: &mut RadixCounts) {
    if token.is_empty() {
        return;
    }
    let raw = token.trim_matches('_').to_string();
    token.clear();
    if raw.is_empty() {
        return;
    }
    let lower = raw.to_lowercase();
    if let Some(rest) = lower.strip_prefix("0b")
        && !rest.is_empty()
        && rest.chars().all(|c| c == '0' || c == '1')
    {
        out.binary += 1;
        return;
    }
    if let Some(rest) = lower.strip_prefix("0o")
        && !rest.is_empty()
        && rest.chars().all(|c| ('0'..='7').contains(&c))
    {
        out.octal += 1;
        return;
    }
    if let Some(rest) = lower.strip_prefix("0x")
        && !rest.is_empty()
        && rest.chars().all(|c| c.is_ascii_hexdigit())
    {
        out.hex += 1;
        return;
    }
    if lower.len() >= 8 && lower.chars().all(|c| c == '0' || c == '1') {
        out.binary += 1;
    } else if lower.chars().all(|c| c.is_ascii_digit()) {
        out.decimal += 1;
    }
}

pub fn detect(text: &str) -> RadixKind {
    let c = counts(text);
    let mut kinds = Vec::new();
    if c.binary > 0 {
        kinds.push(RadixKind::Binary);
    }
    if c.octal > 0 {
        kinds.push(RadixKind::Octal);
    }
    if c.decimal > 0 {
        kinds.push(RadixKind::Decimal);
    }
    if c.hex > 0 {
        kinds.push(RadixKind::Hex);
    }
    match kinds.len() {
        0 => RadixKind::None,
        1 => kinds[0],
        _ => RadixKind::Mixed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_prefixed_radices() {
        let c = counts("0b1010 0o17 42 0xDEAD");
        assert_eq!(
            c,
            RadixCounts {
                binary: 1,
                octal: 1,
                decimal: 1,
                hex: 1
            }
        );
        assert_eq!(detect("0b1010 0o17 42 0xDEAD"), RadixKind::Mixed);
    }

    #[test]
    fn single_radix_and_none() {
        assert_eq!(detect("0xdeadbeef"), RadixKind::Hex);
        assert_eq!(detect("0b1010"), RadixKind::Binary);
        assert_eq!(detect("0o777"), RadixKind::Octal);
        assert_eq!(detect("12345"), RadixKind::Decimal);
        assert_eq!(detect("hello world"), RadixKind::None);
    }

    #[test]
    fn bare_binary_needs_eight_bits() {
        assert_eq!(detect("10101010"), RadixKind::Binary);
        assert_eq!(detect("1010"), RadixKind::Decimal);
    }
}
