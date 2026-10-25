//! 轻量词法预处理：把注释与字符串内容抹成空格（保留行号与长度）。
//!
//! 目的只有一个：**别把注释或字符串里的 `unsafe` 当成代码**。
//! 这不是完整词法器（Rust 的原始字符串/字符字面量边界很绕），够不上就宁可不抹——
//! 但抹错的代价是漏报，所以这里对字符串做了保守处理并在文档里写明局限。

/// 返回与源码行数等长的“清洁行”。
pub fn scrub(source: &str) -> Vec<String> {
    let bytes: Vec<char> = source.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut index = 0usize;

    #[derive(PartialEq)]
    enum State {
        Code,
        LineComment,
        BlockComment(usize),
        Str,
        RawStr(usize), // 0 表示 r"，n 表示 r#...#n
    }
    let mut state = State::Code;

    while index < bytes.len() {
        let ch = bytes[index];
        match state {
            State::Code => {
                if ch == '/' && bytes.get(index + 1) == Some(&'/') {
                    state = State::LineComment;
                    current.push(' ');
                    current.push(' ');
                    index += 2;
                    continue;
                }
                if ch == '/' && bytes.get(index + 1) == Some(&'*') {
                    state = State::BlockComment(1);
                    current.push(' ');
                    current.push(' ');
                    index += 2;
                    continue;
                }
                if ch == 'r'
                    && let Some((hashes, next)) = raw_string_start(&bytes, index)
                {
                    state = State::RawStr(hashes);
                    for _ in index..next {
                        current.push(' ');
                    }
                    index = next;
                    continue;
                }
                if ch == '"' {
                    state = State::Str;
                    current.push(' ');
                    index += 1;
                    continue;
                }
                if ch == '\n' {
                    out.push(std::mem::take(&mut current));
                } else {
                    current.push(ch);
                }
                index += 1;
            }
            State::LineComment => {
                if ch == '\n' {
                    out.push(std::mem::take(&mut current));
                    state = State::Code;
                } else {
                    current.push(' ');
                }
                index += 1;
            }
            State::BlockComment(depth) => {
                if ch == '/' && bytes.get(index + 1) == Some(&'*') {
                    state = State::BlockComment(depth + 1);
                    current.push(' ');
                    current.push(' ');
                    index += 2;
                    continue;
                }
                if ch == '*' && bytes.get(index + 1) == Some(&'/') {
                    current.push(' ');
                    current.push(' ');
                    index += 2;
                    if depth == 1 {
                        state = State::Code;
                    } else {
                        state = State::BlockComment(depth - 1);
                    }
                    continue;
                }
                if ch == '\n' {
                    out.push(std::mem::take(&mut current));
                } else {
                    current.push(' ');
                }
                index += 1;
            }
            State::Str => {
                if ch == '\\' {
                    current.push(' ');
                    current.push(' ');
                    index += 2;
                    continue;
                }
                if ch == '"' {
                    state = State::Code;
                    current.push(' ');
                    index += 1;
                    continue;
                }
                if ch == '\n' {
                    // 未闭合的普通字符串：换行就认为它结束了，避免整份文件被吃掉
                    out.push(std::mem::take(&mut current));
                    state = State::Code;
                } else {
                    current.push(' ');
                }
                index += 1;
            }
            State::RawStr(hashes) => {
                if ch == '"' && raw_string_ends(&bytes, index, hashes) {
                    for _ in 0..=hashes {
                        current.push(' ');
                    }
                    index += 1 + hashes;
                    state = State::Code;
                    continue;
                }
                if ch == '\n' {
                    out.push(std::mem::take(&mut current));
                } else {
                    current.push(' ');
                }
                index += 1;
            }
        }
    }
    out.push(current);
    out
}

/// `r"` / `r#"` / `r##"` 的起点；返回（井号数量，跳过前缀后的下标）。
fn raw_string_start(bytes: &[char], index: usize) -> Option<(usize, usize)> {
    if bytes.get(index) != Some(&'r') {
        return None;
    }
    let mut cursor = index + 1;
    let mut hashes = 0usize;
    while bytes.get(cursor) == Some(&'#') {
        hashes += 1;
        cursor += 1;
        if hashes > 32 {
            return None;
        }
    }
    if bytes.get(cursor) == Some(&'"') {
        Some((hashes, cursor + 1))
    } else {
        None
    }
}

fn raw_string_ends(bytes: &[char], index: usize, hashes: usize) -> bool {
    if bytes.get(index) != Some(&'"') {
        return false;
    }
    (1..=hashes).all(|offset| bytes.get(index + offset) == Some(&'#'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(source: &str) -> String {
        scrub(source).join("\n")
    }

    #[test]
    fn strips_line_and_block_comments() {
        let text = joined("let a = 1; // unsafe { x }\n/* unsafe { y } */ let b = 2;\n");
        assert!(!text.contains("unsafe"), "{text:?}");
        assert!(text.contains("let a = 1;"));
        assert!(text.contains("let b = 2;"));
    }

    #[test]
    fn strips_strings_and_raw_strings() {
        let text = joined("let s = \"unsafe { a }\"; let r = r#\"unsafe { b }\"#; let q = 1;\n");
        assert!(!text.contains("unsafe"), "{text:?}");
        assert!(text.contains("let q = 1;"), "{text:?}");
    }

    #[test]
    fn keeps_line_count_and_real_code() {
        let lines = scrub("fn a() {}\nunsafe { go(); }\n");
        assert_eq!(lines.len(), 3, "末尾换行会多一行空行");
        assert!(lines[1].contains("unsafe"));
    }

    #[test]
    fn nested_block_comments_do_not_leak() {
        let text = joined("/* a /* b */ unsafe { c } */ let d = 1;\n");
        assert!(!text.contains("unsafe"), "{text:?}");
        assert!(text.contains("let d = 1;"));
    }
}
