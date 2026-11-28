//! JSON 语法解析：保留原文里的数字、字符串转义，以及**跨行/空行**这两条格式线索。
//!
//! 不保留的东西：键序（保留）、空白（除跨行/空行线索）、注释（JSON 没有）。

use crate::{Field, FormatError, Node, Obj, Seq, Value};

pub(crate) fn parse_document(source: &str) -> Result<Value, FormatError> {
    Parser::new(source).parse_document()
}

struct Parser<'a> {
    bytes: &'a [u8],
    source: &'a str,
    at: usize,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            bytes: source.as_bytes(),
            source,
            at: 0,
        }
    }

    fn parse_document(mut self) -> Result<Value, FormatError> {
        self.skip_ws();
        let value = self.parse_value()?;
        self.skip_ws();
        if self.at != self.bytes.len() {
            return Err(self.syntax("trailing content"));
        }
        Ok(value)
    }

    fn syntax(&self, message: &str) -> FormatError {
        FormatError::Syntax {
            offset: self.at,
            message: message.to_owned(),
        }
    }

    /// 跳过空白并返回其中的换行数；调用方用它判断"跨行"与"空行"。
    fn skip_ws(&mut self) -> usize {
        let mut newlines = 0;
        while self.at < self.bytes.len() && self.bytes[self.at].is_ascii_whitespace() {
            if self.bytes[self.at] == b'\n' {
                newlines += 1;
            }
            self.at += 1;
        }
        newlines
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn expect(&mut self, byte: u8) -> Result<(), FormatError> {
        if self.peek() == Some(byte) {
            self.at += 1;
            Ok(())
        } else {
            Err(self.syntax(&format!("expected {}", byte as char)))
        }
    }

    fn parse_value(&mut self) -> Result<Value, FormatError> {
        match self.peek() {
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b'"') => Ok(Value::String(self.parse_string()?)),
            Some(b't') => self.parse_literal("true", Value::Bool(true)),
            Some(b'f') => self.parse_literal("false", Value::Bool(false)),
            Some(b'n') => self.parse_literal("null", Value::Null),
            Some(byte) if byte == b'-' || byte.is_ascii_digit() => self.parse_number(),
            Some(_) => Err(self.syntax("unexpected byte")),
            None => Err(self.syntax("unexpected end of input")),
        }
    }

    fn parse_literal(&mut self, text: &str, value: Value) -> Result<Value, FormatError> {
        if self.source[self.at..].starts_with(text) {
            self.at += text.len();
            Ok(value)
        } else {
            Err(self.syntax(&format!("expected {text}")))
        }
    }

    fn parse_number(&mut self) -> Result<Value, FormatError> {
        let start = self.at;
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        while matches!(self.peek(), Some(byte) if byte.is_ascii_digit()) {
            self.at += 1;
        }
        if self.peek() == Some(b'.') {
            self.at += 1;
            while matches!(self.peek(), Some(byte) if byte.is_ascii_digit()) {
                self.at += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.at += 1;
            }
            while matches!(self.peek(), Some(byte) if byte.is_ascii_digit()) {
                self.at += 1;
            }
        }
        if start == self.at {
            return Err(self.syntax("expected number"));
        }
        Ok(Value::Number(self.source[start..self.at].to_owned()))
    }

    /// 字符串保留原始字面量（含两侧引号），只做合法性校验。
    fn parse_string(&mut self) -> Result<String, FormatError> {
        let start = self.at;
        self.expect(b'"')?;
        loop {
            match self.peek() {
                None => return Err(self.syntax("unterminated string")),
                Some(b'"') => {
                    self.at += 1;
                    return Ok(self.source[start..self.at].to_owned());
                }
                Some(b'\\') => {
                    self.at += 1;
                    match self.peek() {
                        Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => {
                            self.at += 1;
                        }
                        Some(b'u') => {
                            self.at += 1;
                            for _ in 0..4 {
                                match self.peek() {
                                    Some(byte) if byte.is_ascii_hexdigit() => self.at += 1,
                                    _ => return Err(self.syntax("bad \\u escape")),
                                }
                            }
                        }
                        _ => return Err(self.syntax("bad escape")),
                    }
                }
                Some(byte) if byte < 0x20 => return Err(self.syntax("control byte in string")),
                Some(_) => self.at += 1,
            }
        }
    }

    fn parse_array(&mut self) -> Result<Value, FormatError> {
        self.expect(b'[')?;
        let mut multiline = self.skip_ws() > 0;
        let mut items = Vec::new();
        if self.peek() == Some(b']') {
            self.at += 1;
            return Ok(Value::Array(Seq { items, multiline }));
        }
        loop {
            let trivia = self.skip_ws();
            multiline |= trivia > 0;
            let blank_before = trivia >= 2;
            let value = self.parse_value()?;
            items.push(Node {
                value,
                blank_before,
            });
            multiline |= self.skip_ws() > 0;
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b']') => {
                    self.at += 1;
                    return Ok(Value::Array(Seq { items, multiline }));
                }
                _ => return Err(self.syntax("expected , or ]")),
            }
        }
    }

    fn parse_object(&mut self) -> Result<Value, FormatError> {
        self.expect(b'{')?;
        let mut multiline = self.skip_ws() > 0;
        let mut fields = Vec::new();
        if self.peek() == Some(b'}') {
            self.at += 1;
            return Ok(Value::Object(Obj { fields, multiline }));
        }
        loop {
            let trivia = self.skip_ws();
            multiline |= trivia > 0;
            let blank_before = trivia >= 2;
            let key = self.parse_string()?;
            self.skip_ws();
            self.expect(b':')?;
            self.skip_ws();
            let value = self.parse_value()?;
            fields.push(Field {
                key,
                value,
                blank_before,
            });
            multiline |= self.skip_ws() > 0;
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b'}') => {
                    self.at += 1;
                    return Ok(Value::Object(Obj { fields, multiline }));
                }
                _ => return Err(self.syntax("expected , or }")),
            }
        }
    }
}
