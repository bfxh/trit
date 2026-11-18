//! 表达式语言的词法层：直接在规则文件**原文**上重新切词。旧词法
//! （`yaraparse/lex.rs`）把 `+ % & | ^ ~` 吞进标识符、把 `..` 拆成两个
//! 点、字符串只认四种转义且存成 `String`（装不下非 UTF-8 字节）——这些
//! 对旧子集无害，对表达式语言全是坑，所以另起一份而不是去改它。
//!
//! 字面量规则（探针定案）：十进制 / `0x` / `0o`，`KB` `MB` 后缀仅大写且只
//! 跟十进制；超出 int64 = 官方编译错误；字符串转义只有
//! `\t \n \r \" \\ \xNN`，其余是官方编译错误；除法拼作 `\`，`/` 永远是
//! 注释或正则的起点。

#[derive(Debug, Clone, PartialEq)]
pub(super) enum T {
    Ident(String),
    /// `$a` / `$`（for-of 当前串）/ `$a*`（集合通配），保留前缀。
    Pat(String),
    /// `#a` / `#`
    Count(String),
    /// `@a` / `@`
    Offset(String),
    /// `!a` / `!`
    Length(String),
    Int(i64),
    Float,
    Str(Vec<u8>),
    /// 带 `\xNN` / `\r` 转义的字符串字面量——旧词法层会把它读错（它只认
    /// `\n \t \\ \"`，其余转义原样留下字母）。条件提取时归一成 `Str`。
    RawStr(Vec<u8>),
    Regex {
        body: String,
        flags: String,
    },
    /// 运算符与标点。
    P(&'static str),
    /// strings 区的十六进制串 `{ … }`，整体略过。
    Hex,
    /// 官方会报编译错误的 token（非法转义、整数越界、不认识的字符）。
    /// 不让它拖垮整份文件的切词：记下原因，哪条规则的条件里有它，
    /// 哪条规则 `Unsupported`。
    Bad(String),
}

const PUNCT: &[&str] = &[
    "..", "<<", ">>", "<=", ">=", "==", "!=", "(", ")", "[", "]", "{", "}", ",", ".", ":", "=",
    "<", ">", "+", "-", "*", "\\", "%", "&", "|", "^", "~",
];

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    out: Vec<T>,
}

impl Lexer<'_> {
    fn peek(&self, ahead: usize) -> Option<u8> {
        self.src.get(self.pos + ahead).copied()
    }

    fn take_while(&mut self, keep: impl Fn(u8) -> bool) -> &str {
        let start = self.pos;
        while self.peek(0).is_some_and(&keep) {
            self.pos += 1;
        }
        // 只在 ASCII 字符类上切，切片必然落在 UTF-8 边界
        std::str::from_utf8(&self.src[start..self.pos]).unwrap_or("")
    }

    fn skip_past(&mut self, end: &[u8], what: &str) -> Result<(), String> {
        match self.src[self.pos..]
            .windows(end.len())
            .position(|w| w == end)
        {
            Some(at) => {
                self.pos += at + end.len();
                Ok(())
            }
            None => Err(format!("{what}未闭合")),
        }
    }

    fn number(&mut self) -> T {
        let overflow = || T::Bad("整数字面量超出 int64（官方：编译错误）".into());
        let radix = match (self.peek(0), self.peek(1)) {
            (Some(b'0'), Some(b'x')) => 16,
            (Some(b'0'), Some(b'o')) => 8,
            _ => 10,
        };
        if radix != 10 {
            self.pos += 2;
            let digits = self.take_while(|b| (b as char).is_digit(radix));
            return i64::from_str_radix(digits, radix).map_or_else(|_| overflow(), T::Int);
        }
        let value = self.take_while(|b| b.is_ascii_digit()).parse::<i64>();
        if self.peek(0) == Some(b'.') && self.peek(1).is_some_and(|b| b.is_ascii_digit()) {
            self.pos += 1;
            self.take_while(|b| b.is_ascii_digit());
            return T::Float;
        }
        let unit = match (self.peek(0), self.peek(1)) {
            (Some(b'K'), Some(b'B')) => 1024,
            (Some(b'M'), Some(b'B')) => 1024 * 1024,
            _ => return value.map_or_else(|_| overflow(), T::Int),
        };
        self.pos += 2;
        let scaled = value.ok().and_then(|v| v.checked_mul(unit));
        scaled.map_or_else(overflow, T::Int)
    }

    fn string(&mut self) -> Result<T, String> {
        let (mut body, mut bad, mut raw) = (Vec::new(), None, false);
        loop {
            self.pos += 1;
            match self.peek(0) {
                None | Some(b'\n') => return Err("字符串未闭合".into()),
                Some(b'"') => break,
                Some(b'\\') => {
                    self.pos += 1;
                    let hex = self.src.get(self.pos + 1..self.pos + 3);
                    let byte = hex
                        .and_then(|h| std::str::from_utf8(h).ok())
                        .and_then(|text| u8::from_str_radix(text, 16).ok());
                    match (self.peek(0), byte) {
                        (Some(b't'), _) => body.push(b'\t'),
                        (Some(b'n'), _) => body.push(b'\n'),
                        (Some(b'r'), _) => {
                            body.push(b'\r');
                            raw = true;
                        }
                        (Some(b'"'), _) => body.push(b'"'),
                        (Some(b'\\'), _) => body.push(b'\\'),
                        (Some(b'x'), Some(byte)) => {
                            body.push(byte);
                            self.pos += 2;
                            raw = true;
                        }
                        (None | Some(b'\n'), _) => return Err("字符串未闭合".into()),
                        _ => bad = Some("字符串里有官方不认的转义（官方：编译错误）"),
                    }
                }
                Some(byte) => body.push(byte),
            }
        }
        self.pos += 1;
        let good = if raw { T::RawStr(body) } else { T::Str(body) };
        Ok(bad.map_or(good, |why| T::Bad(why.into())))
    }

    /// `/` 之后：到未转义的 `/` 为止（`\/` 是字面斜杠），后随 `i` / `s` 标志。
    fn regex(&mut self) -> Result<T, String> {
        let mut body = Vec::new();
        loop {
            match self.peek(0) {
                None | Some(b'\n') => return Err("正则串未闭合".into()),
                Some(b'/') => break,
                Some(b'\\') => {
                    body.extend(self.src.get(self.pos..self.pos + 2).unwrap_or(b"\\"));
                    self.pos += 2;
                }
                Some(byte) => {
                    body.push(byte);
                    self.pos += 1;
                }
            }
        }
        self.pos += 1;
        let flags = self.take_while(|b| b == b'i' || b == b's').to_string();
        let body = String::from_utf8(body).map_err(|_| "正则串不是合法 UTF-8".to_string())?;
        Ok(T::Regex { body, flags })
    }

    fn sigil(&mut self, make: fn(String) -> T) -> T {
        let start = self.pos;
        self.pos += 1;
        self.take_while(is_word);
        if self.src[start] == b'$' && self.peek(0) == Some(b'*') {
            self.pos += 1;
        }
        make(String::from_utf8_lossy(&self.src[start..self.pos]).into_owned())
    }

    fn run(&mut self) -> Result<(), String> {
        while let Some(byte) = self.peek(0) {
            let token = match byte {
                b if b.is_ascii_whitespace() => {
                    self.pos += 1;
                    continue;
                }
                b'/' if self.peek(1) == Some(b'/') => {
                    self.take_while(|b| b != b'\n');
                    continue;
                }
                b'/' if self.peek(1) == Some(b'*') => {
                    self.pos += 2;
                    self.skip_past(b"*/", "块注释")?;
                    continue;
                }
                b'/' => {
                    self.pos += 1;
                    self.regex()?
                }
                b'"' => self.string()?,
                b'{' if self.out.last() == Some(&T::P("=")) => {
                    self.skip_past(b"}", "十六进制串")?;
                    T::Hex
                }
                b'$' => self.sigil(T::Pat),
                b'#' => self.sigil(T::Count),
                b'@' => self.sigil(T::Offset),
                b'!' if self.peek(1) != Some(b'=') => self.sigil(T::Length),
                b if b.is_ascii_digit() => self.number(),
                b if is_word(b) => T::Ident(self.take_while(is_word).to_string()),
                _ => {
                    let rest = &self.src[self.pos..];
                    match PUNCT.iter().find(|p| rest.starts_with(p.as_bytes())) {
                        Some(punct) => {
                            self.pos += punct.len();
                            T::P(punct)
                        }
                        None => {
                            self.pos += 1;
                            T::Bad(format!("无法识别的字节 0x{byte:02x}"))
                        }
                    }
                }
            };
            self.out.push(token);
        }
        Ok(())
    }
}

/// 整个规则文件 → token 流。
pub(super) fn lex(text: &str) -> Result<Vec<T>, String> {
    let mut lexer = Lexer {
        src: text.as_bytes(),
        pos: 0,
        out: Vec::new(),
    };
    lexer.run()?;
    Ok(lexer.out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(text: &str) -> T {
        let mut tokens = lex(text).expect("可切词");
        assert_eq!(tokens.len(), 1, "{text} 应切成单个 token：{tokens:?}");
        tokens.remove(0)
    }

    #[test]
    fn numbers_follow_probed_literal_rules() {
        assert_eq!(one("0x7FFFFFFFFFFFFFFF"), T::Int(i64::MAX));
        assert_eq!(one("0o17"), T::Int(15));
        assert_eq!(one("1KB"), T::Int(1024));
        assert_eq!(one("2MB"), T::Int(2 * 1024 * 1024));
        assert_eq!(one("1.5"), T::Float);
        // 超出 int64 = 官方编译错误
        assert!(matches!(one("9223372036854775808"), T::Bad(_)));
        assert!(matches!(one("0xFFFFFFFFFFFFFFFF"), T::Bad(_)));
        assert!(matches!(one("9007199254740993KB"), T::Bad(_)));
        // 小写后缀 / 十六进制带后缀：数字后面多出一个标识符（解析层会拒）
        assert_eq!(lex("1kb").expect("可切词").len(), 2);
        assert_eq!(lex("0x10KB").expect("可切词").len(), 2);
        // 区间的两个点不会被当成小数点
        let range = lex("(0..3)").expect("可切词");
        assert_eq!(range[1..4], [T::Int(0), T::P(".."), T::Int(3)]);
    }

    #[test]
    fn strings_keep_raw_bytes_and_reject_unknown_escapes() {
        assert_eq!(one(r#""\t\n\"\\""#), T::Str(b"\t\n\"\\".to_vec()));
        // 旧词法层读不对的转义单独标记（分流用）
        assert_eq!(one(r#""a\rb""#), T::RawStr(b"a\rb".to_vec()));
        assert_eq!(one(r#""\x00\xff\x41""#), T::RawStr(vec![0, 0xff, 0x41]));
        assert_eq!(one("\"é\""), T::Str(vec![0xc3, 0xa9]));
        // 非法转义不拖垮整份文件：记成 Bad，后面的 token 照常切
        let tokens = lex(r#""\q" "\x4" "ok""#).expect("可切词");
        assert!(matches!(tokens[..2], [T::Bad(_), T::Bad(_)]));
        assert_eq!(tokens[2], T::Str(b"ok".to_vec()));
        assert!(matches!(one("\u{b}"), T::Bad(_)));
        assert!(matches!(
            lex("é").expect("可切词")[..],
            [T::Bad(_), T::Bad(_)]
        ));
        // 未闭合才是整份文件级的错误（旧词法层同样如此）
        assert!(lex("\"abc").is_err());
        assert!(lex("\"abc\\").is_err());
    }

    #[test]
    fn operators_sigils_and_regexes() {
        let ops = lex(r"1 \ 2 % 3 << 4 >> 5 & 6 | 7 ^ ~8 != 9").expect("可切词");
        let puncts: Vec<_> = ops
            .iter()
            .filter_map(|t| if let T::P(p) = t { Some(*p) } else { None })
            .collect();
        assert_eq!(puncts, ["\\", "%", "<<", ">>", "&", "|", "^", "~", "!="]);
        assert_eq!(one("$a*"), T::Pat("$a*".into()));
        assert_eq!(one("#"), T::Count("#".into()));
        assert_eq!(one("@a1"), T::Offset("@a1".into()));
        assert_eq!(one("!a"), T::Length("!a".into()));
        let regex = one(r"/a\/b/is");
        assert_eq!(
            regex,
            T::Regex {
                body: r"a\/b".into(),
                flags: "is".into()
            }
        );
        // `/` 不是除法：`4 / 2 == 2` 把后半截吞成未闭合的正则
        assert!(lex("4 / 2 == 2").is_err());
        // 注释与十六进制串整体略过
        let rule =
            lex("strings: $h = { AA ?? [2-4] (BB|CC) } // c\n /* x */ condition").expect("可切词");
        assert_eq!(rule[3..], [T::P("="), T::Hex, T::Ident("condition".into())]);
    }
}
