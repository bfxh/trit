//! `.tri` 的最小词法扫描：只为 LSP 的悬停、补全、符号定位服务。
//!
//! 不是第二套解析器：这里只切 token 和记录运算符的元数据（拼写、优先级、结合性、
//! 一个可交给 [`trit_core::Parser`] 复算的样例）。语义永远由 `trit-core`/`trit-eval`
//! 给出；`operators_round_trip` 测试保证每个拼写都真能被解析器接受。

/// token 类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// 运算符（含 `!`/`&`/`|` 等）。
    Operator,
    /// 标识符（可能是变量，也可能是 `true`/`X` 这类常量词）。
    Ident,
    /// 字面常量（`0`/`1`/`?` 或常量词）。
    Constant,
    /// 左括号。
    LParen,
    /// 右括号。
    RParen,
    /// 无法识别的字符。
    Unknown,
}

/// 一个 token，`start`/`end` 是字节偏移（半开）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    /// 类别。
    pub kind: Kind,
    /// 起始字节。
    pub start: usize,
    /// 结束字节（不含）。
    pub end: usize,
    /// 原文。
    pub text: String,
}

/// 运算符元数据。
#[derive(Clone, Copy, Debug)]
pub struct OpInfo {
    /// 全部可写拼写（第一个是补全里用的主拼写）。
    pub spellings: &'static [&'static str],
    /// 中文名。
    pub name: &'static str,
    /// 目数（1 或 2）。
    pub arity: u8,
    /// 优先级（数字越大越紧；与 `trit_core::parser` 的分层一致）。
    pub precedence: u8,
    /// 结合性说明。
    pub assoc: &'static str,
    /// 用于真值表复算的样例表达式。
    pub sample: &'static str,
}

/// 运算符表。优先级：`<=>` 0 < `=>` 1 < `|` 2 < `^` 3 < `&` 4 < `!` 5。
pub const OPS: &[OpInfo] = &[
    OpInfo {
        spellings: &["!"],
        name: "否定",
        arity: 1,
        precedence: 5,
        assoc: "前缀",
        sample: "!a",
    },
    OpInfo {
        spellings: &["&", "&&", "∧"],
        name: "与（合取）",
        arity: 2,
        precedence: 4,
        assoc: "左结合",
        sample: "a & b",
    },
    OpInfo {
        spellings: &["⊕", "^"],
        name: "强加法（异或）",
        arity: 2,
        precedence: 3,
        assoc: "左结合",
        sample: "a ^ b",
    },
    OpInfo {
        spellings: &["|", "||", "∨"],
        name: "或（析取）",
        arity: 2,
        precedence: 2,
        assoc: "左结合",
        sample: "a | b",
    },
    OpInfo {
        spellings: &["=>", "→", "⇒"],
        name: "蕴含",
        arity: 2,
        precedence: 1,
        assoc: "右结合",
        sample: "a => b",
    },
    OpInfo {
        spellings: &["<=>", "≡", "=="],
        name: "等价",
        arity: 2,
        precedence: 0,
        assoc: "左结合",
        sample: "a <=> b",
    },
];

impl OpInfo {
    /// 补全时插入的主拼写。
    pub fn primary(self) -> &'static str {
        self.spellings[0]
    }
}

/// 查一个运算符原文对应的元数据。
pub fn operator(text: &str) -> Option<&'static OpInfo> {
    OPS.iter().find(|op| op.spellings.contains(&text))
}

/// 最长匹配用：多字符在前，单字符在后。
const OP_SPELLINGS: &[&str] = &[
    "<=>", "&&", "||", "=>", "==", "!", "~", "¬", "&", "|", "^", "⊕", "∨", "∧", "→", "⇒", "≡",
];

/// 解析器把整词当作常量的标识符。
const CONST_WORDS: &[&str] = &[
    "True", "true", "TRUE", "False", "false", "FALSE", "X", "x", "Unknown", "unknown",
];

/// 扫一遍源码。`#` 到行尾按注释跳过（`.tri` 的注释约定）。
pub fn tokens(text: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < text.len() {
        let ch = next_char(text, i);
        if ch.is_whitespace() {
            i += ch.len_utf8();
            continue;
        }
        if ch == '#' {
            while i < text.len() {
                let c = next_char(text, i);
                i += c.len_utf8();
                if c == '\n' {
                    break;
                }
            }
            continue;
        }
        let rest = &text[i..];
        if let Some(spelling) = OP_SPELLINGS.iter().find(|s| rest.starts_with(**s)) {
            out.push(Token {
                kind: Kind::Operator,
                start: i,
                end: i + spelling.len(),
                text: (*spelling).to_string(),
            });
            i += spelling.len();
            continue;
        }
        if ch == '(' || ch == ')' {
            out.push(Token {
                kind: if ch == '(' {
                    Kind::LParen
                } else {
                    Kind::RParen
                },
                start: i,
                end: i + 1,
                text: ch.to_string(),
            });
            i += 1;
            continue;
        }
        if is_ident_start(ch) {
            let start = i;
            let mut j = i;
            while j < text.len() {
                let c = next_char(text, j);
                if is_ident_char(c) {
                    j += c.len_utf8();
                } else {
                    break;
                }
            }
            let word = &text[start..j];
            let kind = if CONST_WORDS.contains(&word) {
                Kind::Constant
            } else {
                Kind::Ident
            };
            out.push(Token {
                kind,
                start,
                end: j,
                text: word.to_string(),
            });
            i = j;
            continue;
        }
        if matches!(ch, '0' | '1' | '?') {
            out.push(Token {
                kind: Kind::Constant,
                start: i,
                end: i + 1,
                text: ch.to_string(),
            });
            i += 1;
            continue;
        }
        out.push(Token {
            kind: Kind::Unknown,
            start: i,
            end: i + ch.len_utf8(),
            text: ch.to_string(),
        });
        i += ch.len_utf8();
    }
    out
}

fn next_char(text: &str, i: usize) -> char {
    text[i..].chars().next().expect("i 在 char 边界上")
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(Kind, String)> {
        tokens(text).into_iter().map(|t| (t.kind, t.text)).collect()
    }

    #[test]
    fn splits_expression() {
        let got = kinds("(a & b) | !c");
        let got: Vec<(Kind, &str)> = got.iter().map(|(k, s)| (*k, s.as_str())).collect();
        assert_eq!(
            got,
            vec![
                (Kind::LParen, "("),
                (Kind::Ident, "a"),
                (Kind::Operator, "&"),
                (Kind::Ident, "b"),
                (Kind::RParen, ")"),
                (Kind::Operator, "|"),
                (Kind::Operator, "!"),
                (Kind::Ident, "c"),
            ]
        );
    }

    #[test]
    fn longest_match_wins() {
        let got = kinds("a <=> b => c && d || e == f");
        let ops: Vec<&str> = got
            .iter()
            .filter(|(k, _)| *k == Kind::Operator)
            .map(|(_, s)| s.as_str())
            .collect();
        assert_eq!(ops, vec!["<=>", "=>", "&&", "||", "=="]);
    }

    #[test]
    fn comments_and_constants() {
        let got = kinds("# 注释\n0 & 1 | X && true");
        let constants: Vec<&str> = got
            .iter()
            .filter(|(k, _)| *k == Kind::Constant)
            .map(|(_, s)| s.as_str())
            .collect();
        assert_eq!(constants, vec!["0", "1", "X", "true"]);
    }

    #[test]
    fn unicode_operator_offsets_are_bytes() {
        let text = "a ⊕ b";
        let op = tokens(text)
            .into_iter()
            .find(|t| t.kind == Kind::Operator)
            .expect("有运算符");
        assert_eq!(&text[op.start..op.end], "⊕");
        assert_eq!(op.end - op.start, 3);
    }

    #[test]
    fn operators_round_trip() {
        // 每个拼写都必须真能被解析器接受，且样例能求值——防止 LSP 和语言分叉。
        for op in OPS {
            for spelling in op.spellings {
                assert_eq!(
                    operator(spelling).map(|o| o.name),
                    Some(op.name),
                    "{spelling} 查不到"
                );
            }
            trit_core::Parser::parse(op.sample)
                .unwrap_or_else(|e| panic!("样例 {:?} 解析失败：{e}", op.sample));
        }
    }
}
