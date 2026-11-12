//! 词法层（从 yaraparse.rs 拆出守模块预算）：规则文件文本 → token 流。

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Tok {
    /// 标识符/关键字/数字（含 `$a` 引用、`0x10` 十六进制）。
    Word(String),
    /// 双引号字符串（已去引号，转义已还原）。
    Str(String),
    /// 正则串 `/…/`——词法层认出，语义层判 Unsupported。
    Regex(String),
    Sym(char),
}
pub(super) fn lex(text: &str) -> Result<Vec<Tok>, String> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            // Unicode 空白全集（F-02/M15：ASCII 四个会让 \u{b} 死循环——yara_lexer 钉死）
            c if c.is_whitespace() => {
                chars.next();
            }
            '/' => {
                chars.next();
                match chars.peek() {
                    Some('/') => {
                        chars.next();
                        while let Some(&n) = chars.peek() {
                            chars.next();
                            if n == '\n' {
                                break;
                            }
                        }
                    }
                    Some('*') => {
                        chars.next();
                        let mut closed = false;
                        while let Some(&n) = chars.peek() {
                            chars.next();
                            if n == '*' && chars.peek() == Some(&'/') {
                                chars.next();
                                closed = true;
                                break;
                            }
                        }
                        if !closed {
                            return Err("块注释未闭合".into());
                        }
                    }
                    _ => {
                        // 正则串：到未转义的 '/' 为止
                        let mut body = String::new();
                        let mut closed = false;
                        while let Some(&n) = chars.peek() {
                            chars.next();
                            if n == '\\' {
                                if let Some(&e) = chars.peek() {
                                    chars.next();
                                    body.push('\\');
                                    body.push(e);
                                }
                                continue;
                            }
                            if n == '/' {
                                closed = true;
                                break;
                            }
                            body.push(n);
                        }
                        if !closed {
                            return Err("正则串未闭合".into());
                        }
                        out.push(Tok::Regex(body));
                    }
                }
            }
            '"' => {
                chars.next();
                let mut body = String::new();
                let mut closed = false;
                while let Some(&n) = chars.peek() {
                    chars.next();
                    match n {
                        '\\' => {
                            if let Some(&e) = chars.peek() {
                                chars.next();
                                body.push(match e {
                                    'n' => '\n',
                                    't' => '\t',
                                    '\\' => '\\',
                                    '"' => '"',
                                    other => other,
                                });
                            }
                        }
                        '"' => {
                            closed = true;
                            break;
                        }
                        other => body.push(other),
                    }
                }
                if !closed {
                    return Err("字符串未闭合".into());
                }
                out.push(Tok::Str(body));
            }
            '{' | '}' | '(' | ')' | ':' | '=' | '[' | ']' | '*' | '.' | ',' | '<' | '>' | '!'
            | '-' => {
                chars.next();
                out.push(Tok::Sym(c));
            }
            _ => {
                let mut word = String::new();
                while let Some(&n) = chars.peek() {
                    if n.is_whitespace()
                        || matches!(
                            n,
                            '{' | '}'
                                | '('
                                | ')'
                                | ':'
                                | '='
                                | '"'
                                | '.'
                                | '<'
                                | '>'
                                | '!'
                                | '-'
                                | '['
                                | ']'
                        )
                    {
                        break;
                    }
                    chars.next();
                    word.push(n);
                }
                if !word.is_empty() {
                    out.push(Tok::Word(word));
                }
            }
        }
    }
    Ok(out)
}
