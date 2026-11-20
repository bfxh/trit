//! `scrub` 的实现（从 `lexer.rs` 拆出，守模块行数）。

/// 返回与源码行数等长的“清洁行”。
use super::raw::{raw_string_ends, raw_string_start};

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
                // 字符字面量：`'x'`、`'"'`、`'\n'` 整段抹成空格，否则其中的 `"`
                // 会被当成字符串起点，让同一行的 unsafe 隐身（第 2 轮 U1）。
                if ch == '\''
                    && let Some(len) = char_literal_len(&bytes, index)
                {
                    for _ in 0..len {
                        current.push(' ');
                    }
                    index += len;
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
/// `'x'` / `'"'` / `'\n'` 的长度；生命周期（`'a`、`'static`）返回 `None`。
fn char_literal_len(bytes: &[char], index: usize) -> Option<usize> {
    if *bytes.get(index + 1)? == '\\' {
        let end = bytes
            .get(index + 2..index + 6)?
            .iter()
            .position(|c| *c == '\'')?;
        return Some(3 + end);
    }
    matches!(bytes.get(index + 2), Some(&'\'')).then_some(3)
}
