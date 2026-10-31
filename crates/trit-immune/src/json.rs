//! 极简 JSON 解析器（零依赖）：为编译器诊断（`rustc --message-format=json`）、
//! 外部扫描器输出（Semgrep/CodeQL/ADV）等 JSON 数据源服务。
//!
//! 递归下降 + 深度上限（防对抗性深嵌套栈溢出）；数字按 f64（诊断数据不需要
//! 精确整数语义）；字符串转义全支持（含 UTF-16 代理对）；尾随内容拒绝。

/// 解析后的 JSON 值。
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    /// 取对象字段。
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_arr(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }
}

/// 解析一段 JSON 文本；尾随内容视为错误。
pub fn parse(text: &str) -> Result<Json, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut p = Parser { b: &chars, i: 0, depth: 0 };
    p.skip_ws();
    let v = p.value()?;
    p.skip_ws();
    if p.i != p.b.len() {
        return Err(format!("尾随内容（位置 {}）", p.i));
    }
    Ok(v)
}

const MAX_DEPTH: usize = 128;

struct Parser<'a> {
    b: &'a [char],
    i: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<char> {
        self.b.get(self.i).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.i += 1;
        }
        c
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
            self.i += 1;
        }
    }

    fn expect(&mut self, c: char) -> Result<(), String> {
        if self.peek() == Some(c) {
            self.i += 1;
            Ok(())
        } else {
            Err(format!("期望 {c:?}，遇到 {:?}", self.peek()))
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            self.depth -= 1;
            return Err("嵌套过深".into());
        }
        let v = match self.peek() {
            Some('{') => self.object(),
            Some('[') => self.array(),
            Some('"') => self.string().map(Json::Str),
            Some('t') => self.lit("true", Json::Bool(true)),
            Some('f') => self.lit("false", Json::Bool(false)),
            Some('n') => self.lit("null", Json::Null),
            Some(c) if c == '-' || c.is_ascii_digit() => self.number(),
            other => Err(format!("非法起始字符 {other:?}")),
        };
        self.depth -= 1;
        v
    }

    fn lit(&mut self, word: &str, v: Json) -> Result<Json, String> {
        for c in word.chars() {
            self.expect(c)?;
        }
        Ok(v)
    }

    fn object(&mut self) -> Result<Json, String> {
        self.expect('{')?;
        let mut out = Vec::new();
        self.skip_ws();
        if self.peek() == Some('}') {
            self.i += 1;
            return Ok(Json::Obj(out));
        }
        loop {
            self.skip_ws();
            let k = self.string()?;
            self.skip_ws();
            self.expect(':')?;
            self.skip_ws();
            let v = self.value()?;
            out.push((k, v));
            self.skip_ws();
            match self.bump() {
                Some(',') => continue,
                Some('}') => return Ok(Json::Obj(out)),
                other => return Err(format!("期望 , 或 }}，遇到 {other:?}")),
            }
        }
    }

    fn array(&mut self) -> Result<Json, String> {
        self.expect('[')?;
        let mut out = Vec::new();
        self.skip_ws();
        if self.peek() == Some(']') {
            self.i += 1;
            return Ok(Json::Arr(out));
        }
        loop {
            self.skip_ws();
            out.push(self.value()?);
            self.skip_ws();
            match self.bump() {
                Some(',') => continue,
                Some(']') => return Ok(Json::Arr(out)),
                other => return Err(format!("期望 , 或 ]，遇到 {other:?}")),
            }
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect('"')?;
        let mut s = String::new();
        loop {
            match self.bump() {
                None => return Err("字符串未闭合".into()),
                Some('"') => return Ok(s),
                Some('\\') => match self.bump() {
                    Some('"') => s.push('"'),
                    Some('\\') => s.push('\\'),
                    Some('/') => s.push('/'),
                    Some('b') => s.push('\u{8}'),
                    Some('f') => s.push('\u{c}'),
                    Some('n') => s.push('\n'),
                    Some('r') => s.push('\r'),
                    Some('t') => s.push('\t'),
                    Some('u') => {
                        let hi = self.hex4()?;
                        let ch = if (0xD800..0xDC00).contains(&hi) {
                            // UTF-16 代理对
                            self.expect('\\')?;
                            self.expect('u')?;
                            let lo = self.hex4()?;
                            if !(0xDC00..0xE000).contains(&lo) {
                                return Err("非法低位代理".into());
                            }
                            let cp = 0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00);
                            char::from_u32(cp).ok_or("非法码点")?
                        } else {
                            char::from_u32(hi).ok_or("非法码点")?
                        };
                        s.push(ch);
                    }
                    other => return Err(format!("非法转义 {other:?}")),
                },
                Some(c) if (c as u32) < 0x20 => {
                    return Err("字符串含裸控制字符（必须转义）".into())
                }
                Some(c) => s.push(c),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let mut v = 0u32;
        for _ in 0..4 {
            let c = self.bump().ok_or("\\u 截断")?;
            let d = c
                .to_digit(16)
                .ok_or_else(|| format!("非法十六进制 {c:?}"))?;
            v = v * 16 + d;
        }
        Ok(v)
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.i;
        if self.peek() == Some('-') {
            self.i += 1;
        }
        while matches!(self.peek(), Some(c) if c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'))
        {
            self.i += 1;
        }
        let s: String = self.b[start..self.i].iter().collect();
        s.parse::<f64>()
            .map(Json::Num)
            .map_err(|_| format!("非法数字 {s:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_objects_and_arrays() {
        let v = parse(r#"{"a":1,"b":[true,null,"x"],"c":{"d":-2.5e2}}"#).unwrap();
        assert_eq!(v.get("a").unwrap().as_f64(), Some(1.0));
        assert_eq!(v.get("b").unwrap().as_arr().unwrap().len(), 3);
        assert_eq!(
            v.get("c").unwrap().get("d").unwrap().as_f64(),
            Some(-250.0)
        );
    }

    #[test]
    fn escapes_and_surrogate_pairs() {
        let v = parse(r#""a\"b\\c\/d\n\t😀""#).unwrap();
        assert_eq!(v.as_str(), Some("a\"b\\c/d\n\t😀"));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("").is_err());
        assert!(parse("{").is_err());
        assert!(parse("{\"a\":1} trailing").is_err());
        assert!(parse("\"unclosed").is_err());
        assert!(parse("{\"a\" 1}").is_err());
        assert!(parse("[1,]").is_err());
        assert!(parse("tru").is_err());
        assert!(parse("-").is_err());
        // 深度上限
        let deep = "[".repeat(200) + &"]".repeat(200);
        assert!(parse(&deep).is_err());
        // 裸控制字符（非转义）拒绝
        assert!(parse("\"a\nb\"").is_err());
    }
}
