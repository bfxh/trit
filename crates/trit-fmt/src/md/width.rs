//! 显示宽度：Markdown 表格对齐要按**显示列宽**而不是字符数算。
//!
//! 口径与 prettier 的 `string-width` 对齐到够用为止：东亚宽字符算 2，
//! 组合附加符号算 0，其余算 1。emoji 按 2（prettier 同款），这里只取常用区段。

/// 单个字符的显示宽度。
pub(crate) fn char_width(c: char) -> usize {
    let code = c as u32;
    if is_zero_width(code) {
        return 0;
    }
    if is_wide(code) {
        return 2;
    }
    1
}

/// 字符串的显示宽度。
pub(crate) fn str_width(text: &str) -> usize {
    text.chars().map(char_width).sum()
}

fn is_zero_width(code: u32) -> bool {
    matches!(code,
        0x0300..=0x036F | 0x0483..=0x0489 | 0x0591..=0x05BD | 0x0610..=0x061A
        | 0x064B..=0x065F | 0x0670 | 0x06D6..=0x06DC | 0x0E31 | 0x0E34..=0x0E3A
        | 0x0E47..=0x0E4E | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20F0
        | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F | 0x1F3FB..=0x1F3FF)
}

fn is_wide(code: u32) -> bool {
    matches!(code,
        0x1100..=0x115F
        | 0x2600..=0x27BF
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xA960..=0xA97F
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE10..=0xFE19
        | 0xFE30..=0xFE6B
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1F64F
        | 0x1F900..=0x1F9FF
        | 0x20000..=0x2FFFD
        | 0x30000..=0x3FFFD)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_is_one() {
        assert_eq!(str_width("abc"), 3);
    }

    #[test]
    fn cjk_is_two() {
        assert_eq!(str_width("中文"), 4);
        // `| 名称 |`：两个竖线 + 两个空格各算 1，两个汉字各算 2。
        assert_eq!(str_width("| 名称 |"), 8);
    }

    #[test]
    fn combining_marks_are_zero() {
        assert_eq!(str_width("e\u{0301}"), 1);
    }
}
