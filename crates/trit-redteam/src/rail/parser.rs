use std::collections::BTreeMap;

use anyhow::{Result, bail};

struct Parser<'a> {
    chars: Vec<char>,
    index: usize,
    env: &'a BTreeMap<String, u8>,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.index).copied()
    }

    fn rest(&self) -> String {
        self.chars[self.index..].iter().collect()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(' ') | Some('\t')) {
            self.index += 1;
        }
    }

    fn parse_or(&mut self) -> Result<u8> {
        let mut value = self.parse_and()?;
        loop {
            self.skip_ws();
            if self.peek() == Some('|') {
                self.index += 1;
                value |= self.parse_and()?;
            } else {
                return Ok(value);
            }
        }
    }

    fn parse_and(&mut self) -> Result<u8> {
        let mut value = self.parse_unary()?;
        loop {
            self.skip_ws();
            if self.peek() == Some('&') {
                self.index += 1;
                value &= self.parse_unary()?;
            } else {
                return Ok(value);
            }
        }
    }

    fn parse_unary(&mut self) -> Result<u8> {
        self.skip_ws();
        match self.peek() {
            Some('~') | Some('!') => {
                self.index += 1;
                let value = self.parse_unary()?;
                Ok(1 - value)
            }
            Some('(') => {
                self.index += 1;
                let value = self.parse_or()?;
                self.skip_ws();
                if self.peek() != Some(')') {
                    bail!("位置 {}：缺少右括号", self.index);
                }
                self.index += 1;
                Ok(value)
            }
            _ => self.parse_atom(),
        }
    }

    fn parse_atom(&mut self) -> Result<u8> {
        self.skip_ws();
        let rest = self.rest();
        if rest.starts_with("1'b0") {
            self.index += 4;
            return Ok(0);
        }
        if rest.starts_with("1'b1") {
            self.index += 4;
            return Ok(1);
        }
        let start = self.index;
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' {
                self.index += 1;
            } else {
                break;
            }
        }
        if self.index == start {
            bail!("位置 {start}：表达式在此处无法解析");
        }
        let name: String = self.chars[start..self.index].iter().collect();
        match self.env.get(&name) {
            Some(value) => Ok(*value),
            None => bail!("位置 {start}：未定义信号 {name}"),
        }
    }
}

/// 求值一段门级表达式。
///
/// 这里**不用 panic**：输入是别人生成的网表，畸形写法属于预期输入，必须以
/// `Err` 回到调用方，否则一份坏文件就能让审计器崩掉（见台账 `L56`）。
pub fn eval_expr(text: &str, env: &BTreeMap<String, u8>) -> Result<u8> {
    let mut parser = Parser {
        chars: text.chars().collect(),
        index: 0,
        env,
    };
    let value = parser.parse_or()?;
    // 尾部残留必须报错：`a b` 若静默只算 `a`，分析结论会指向一个不存在的表达式。
    parser.skip_ws();
    if parser.index < parser.chars.len() {
        bail!("位置 {}：表达式后有多余字符", parser.index);
    }
    Ok(value)
}
