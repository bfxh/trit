//! assign 右值表达式：解析、位宽推断与按位展开。

use std::collections::{BTreeMap, BTreeSet};

use trit_core::value::Ternary;

use super::{VerilogError, message};

/// assign 右值的最大嵌套深度。
const MAX_RHS_DEPTH: usize = 256;
/// 单条 assign 里二元运算符的总数上限。
///
/// 平链会构造成左深 AST，`bit_expr` / `synth_v` 再递归遍历它——不设上限时
/// 超长链在**合成阶段**栈溢出（解析本身是循环，深度护栏拦不住）。取值与
/// `MAX_RHS_DEPTH` 同档：测试线程 2 MiB 栈上，左深 256 层 × 2 帧仍有足够余量。
pub(super) const MAX_RHS_OPS: usize = 256;

/// `assign` 右值的小型表达式树（Verilog 语义：`^` 是经典异或）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum VExpr {
    Var(String),
    Const(Ternary),
    /// 带位宽的常量；`bits[0]` 是最低位。
    ConstBits {
        width: usize,
        bits: Vec<Ternary>,
    },
    Not(Box<VExpr>),
    And(Box<VExpr>, Box<VExpr>),
    Or(Box<VExpr>, Box<VExpr>),
    Xor(Box<VExpr>, Box<VExpr>),
    /// 拼接 `{a, b, ...}`（低位在右）。
    Concat(Vec<VExpr>),
}

pub(super) struct RhsParser {
    chars: Vec<char>,
    pos: usize,
    depth: usize,
    ops: usize,
}

impl RhsParser {
    pub(super) fn new(text: &str) -> Self {
        Self {
            chars: text.chars().collect(),
            pos: 0,
            depth: 0,
            ops: 0,
        }
    }

    fn enter(&mut self) -> Result<(), VerilogError> {
        self.depth += 1;
        if self.depth > MAX_RHS_DEPTH {
            return Err(VerilogError::limit(format!(
                "assign 表达式嵌套超过 {MAX_RHS_DEPTH} 层"
            )));
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// 每消耗一个二元运算符记一次数；平链的左深 AST 深度由这里封顶。
    fn count_op(&mut self) -> Result<(), VerilogError> {
        self.ops += 1;
        if self.ops > MAX_RHS_OPS {
            return Err(VerilogError::limit(format!(
                "assign 表达式的二元运算超过 {MAX_RHS_OPS} 次"
            )));
        }
        Ok(())
    }

    fn skip_ws(&mut self) {
        while self.pos < self.chars.len() && self.chars[self.pos].is_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += 1;
        Some(ch)
    }

    pub(super) fn parse(mut self) -> Result<VExpr, VerilogError> {
        let expr = self.parse_or()?;
        self.skip_ws();
        if self.pos != self.chars.len() {
            return Err(message(format!(
                "assign 表达式在位置 {} 处有多余字符",
                self.pos
            )));
        }
        Ok(expr)
    }

    fn parse_or(&mut self) -> Result<VExpr, VerilogError> {
        let mut left = self.parse_xor()?;
        loop {
            self.skip_ws();
            if self.peek() == Some('|') {
                // 复审 M02：`||` 是逻辑或（1 bit 结果），不是逐位 `|`。
                // 子集不实现逻辑运算，就显式拒绝，不能静默按位解释。
                if self.chars.get(self.pos + 1).copied() == Some('|') {
                    return Err(VerilogError::unsupported(
                        "逻辑或 `||` 不在 Verilog 子集；请用逐位 `|` 或显式三值判断",
                    ));
                }
                self.pos += 1;
                self.count_op()?;
                let right = self.parse_xor()?;
                left = VExpr::Or(Box::new(left), Box::new(right));
            } else {
                return Ok(left);
            }
        }
    }

    fn parse_xor(&mut self) -> Result<VExpr, VerilogError> {
        let mut left = self.parse_and()?;
        loop {
            self.skip_ws();
            let negated = match (self.peek(), self.chars.get(self.pos + 1).copied()) {
                (Some('~'), Some('^')) | (Some('^'), Some('~')) => {
                    self.pos += 2;
                    true
                }
                (Some('^'), _) => {
                    self.pos += 1;
                    false
                }
                _ => return Ok(left),
            };
            self.count_op()?;
            let right = self.parse_and()?;
            let mut expr = VExpr::Xor(Box::new(left), Box::new(right));
            if negated {
                expr = VExpr::Not(Box::new(expr));
            }
            left = expr;
        }
    }

    fn parse_and(&mut self) -> Result<VExpr, VerilogError> {
        let mut left = self.parse_unary()?;
        loop {
            self.skip_ws();
            if self.peek() == Some('&') {
                // 复审 M02：`&&` 是逻辑与，结果 1 bit；含 X 时的语义也不同。
                if self.chars.get(self.pos + 1).copied() == Some('&') {
                    return Err(VerilogError::unsupported(
                        "逻辑与 `&&` 不在 Verilog 子集；请用逐位 `&` 或显式三值判断",
                    ));
                }
                self.pos += 1;
                self.count_op()?;
                let right = self.parse_unary()?;
                left = VExpr::And(Box::new(left), Box::new(right));
            } else {
                return Ok(left);
            }
        }
    }

    fn parse_unary(&mut self) -> Result<VExpr, VerilogError> {
        self.skip_ws();
        match self.peek() {
            Some('~') | Some('!') => {
                self.pos += 1;
                self.enter()?;
                let inner = self.parse_unary();
                self.leave();
                Ok(VExpr::Not(Box::new(inner?)))
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<VExpr, VerilogError> {
        self.skip_ws();
        match self.peek() {
            Some('{') => {
                // 拼接和括号一样是递归文法分支，必须计入深度——否则
                // `{a,{a,{…}}}` 的嵌套不设防，在解析期就耗尽栈。
                self.enter()?;
                let parsed = self.parse_concat_body();
                self.leave();
                parsed
            }
            Some('(') => {
                self.pos += 1;
                self.enter()?;
                let expr = self.parse_or();
                self.leave();
                let expr = expr?;
                self.skip_ws();
                if self.bump() != Some(')') {
                    return Err(message("assign 表达式缺少 ')'"));
                }
                Ok(expr)
            }
            Some(ch) if ch.is_ascii_digit() => {
                let start = self.pos;
                while let Some(ch) = self.peek() {
                    if ch.is_ascii_alphanumeric() || ch == '\'' || ch == '_' {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                let literal: String = self.chars[start..self.pos].iter().collect();
                parse_literal(&literal)
            }
            Some(ch) if ch.is_ascii_alphabetic() || ch == '_' || ch == '$' => {
                let start = self.pos;
                while let Some(ch) = self.peek() {
                    if ch.is_ascii_alphanumeric() || ch == '_' || ch == '$' {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                let name: String = self.chars[start..self.pos].iter().collect();
                // 位选 `a[3]` 直接解析成位网名 `a[3]`。
                self.skip_ws();
                if self.peek() == Some('[') {
                    self.pos += 1;
                    let index_start = self.pos;
                    while self.peek().is_some_and(|ch| ch.is_ascii_digit()) {
                        self.pos += 1;
                    }
                    let index: String = self.chars[index_start..self.pos].iter().collect();
                    self.skip_ws();
                    if self.bump() != Some(']') {
                        return Err(message("位选缺少 ']'"));
                    }
                    let index: usize = index
                        .parse()
                        .map_err(|_| message(format!("位选 {name}[{index}] 的下标不是数字")))?;
                    return Ok(VExpr::Var(format!("{name}[{index}]")));
                }
                Ok(VExpr::Var(name))
            }
            Some(ch) => Err(message(format!("assign 表达式里出现意外字符 {ch:?}"))),
            None => Err(message("assign 表达式不完整")),
        }
    }

    fn parse_concat_body(&mut self) -> Result<VExpr, VerilogError> {
        self.pos += 1; // '{' 已消费
        let mut items = Vec::new();
        loop {
            items.push(self.parse_or()?);
            self.skip_ws();
            match self.bump() {
                Some(',') => continue,
                Some('}') => return Ok(VExpr::Concat(items)),
                _ => {
                    return Err(VerilogError::unsupported(
                        "拼接缺少 '}'（只支持 {a, b, ...}，不支持复制语法）",
                    ));
                }
            }
        }
    }
}

fn parse_literal(text: &str) -> Result<VExpr, VerilogError> {
    let lowered = text.to_ascii_lowercase();
    if let Some((width_text, rest)) = lowered.split_once('\'') {
        return parse_sized_literal(text, width_text, rest);
    }
    let value = match lowered.as_str() {
        "0" => Ternary::False,
        "1" => Ternary::True,
        "x" | "z" => Ternary::Unknown,
        _ => {
            return Err(VerilogError::unsupported(format!(
                "暂不支持常量 {text:?}（只接受 0/1/X/Z 或 <width>'<b|o|d|h><digits>）"
            )));
        }
    };
    Ok(VExpr::Const(value))
}

fn parse_sized_literal(
    original: &str,
    width_text: &str,
    rest: &str,
) -> Result<VExpr, VerilogError> {
    let width: usize = width_text
        .parse()
        .map_err(|_| message(format!("常量 {original:?} 的位宽不是数字")))?;
    if width == 0 || width > super::MAX_VECTOR_WIDTH {
        return Err(VerilogError::limit(format!(
            "常量 {original:?} 的位宽 {width} 超过上限 {}",
            super::MAX_VECTOR_WIDTH
        )));
    }
    let mut chars = rest.chars();
    let base = chars
        .next()
        .ok_or_else(|| message(format!("常量 {original:?} 缺少进制")))?;
    let digits: String = chars.collect::<String>().replace('_', "");
    if digits.is_empty() {
        return Err(message(format!("常量 {original:?} 缺少数字")));
    }
    let mut bits: Vec<Ternary> = Vec::with_capacity(width);
    match base {
        'b' => {
            for ch in digits.chars().rev() {
                bits.push(match ch {
                    '0' => Ternary::False,
                    '1' => Ternary::True,
                    'x' | 'z' => Ternary::Unknown,
                    _ => {
                        return Err(message(format!(
                            "常量 {original:?} 含非法二进制数字 {ch:?}"
                        )));
                    }
                });
            }
        }
        'o' => expand_radix(&digits, 3, &mut bits)?,
        'h' => expand_radix(&digits, 4, &mut bits)?,
        'd' => {
            let value: u128 = digits.parse().map_err(|_| {
                VerilogError::unsupported(format!(
                    "十进制常量 {original:?} 含非法字符或超过 128 位，请改用 b/o/h"
                ))
            })?;
            for index in 0..width {
                let set = index < 128 && ((value >> index) & 1) == 1;
                bits.push(if set { Ternary::True } else { Ternary::False });
            }
        }
        other => {
            return Err(VerilogError::unsupported(format!(
                "常量 {original:?} 的进制 {other:?} 不支持（只支持 b/o/d/h）"
            )));
        }
    }
    // Verilog 规则：数字不足高位补 0，超出则截断高位。
    bits.resize(width, Ternary::False);
    if width == 1 {
        Ok(VExpr::Const(bits[0]))
    } else {
        Ok(VExpr::ConstBits { width, bits })
    }
}

fn expand_radix(digits: &str, group: usize, bits: &mut Vec<Ternary>) -> Result<(), VerilogError> {
    for ch in digits.chars().rev() {
        if ch == 'x' || ch == 'z' {
            for _ in 0..group {
                bits.push(Ternary::Unknown);
            }
            continue;
        }
        let value = ch
            .to_digit(1u32 << group)
            .ok_or_else(|| message(format!("非法数字 {ch:?}")))?;
        for bit in 0..group {
            bits.push(if (value >> bit) & 1 == 1 {
                Ternary::True
            } else {
                Ternary::False
            });
        }
    }
    Ok(())
}

/// 表达式位宽（标量 1；向量按声明位宽；拼接为各段之和）。
pub(super) fn expr_width(expr: &VExpr, widths: &BTreeMap<String, usize>) -> usize {
    match expr {
        VExpr::Var(name) => widths.get(name).copied().unwrap_or(1),
        VExpr::Const(_) => 1,
        VExpr::ConstBits { width, .. } => *width,
        VExpr::Not(inner) => expr_width(inner, widths),
        VExpr::And(a, b) | VExpr::Or(a, b) | VExpr::Xor(a, b) => {
            expr_width(a, widths).max(expr_width(b, widths))
        }
        VExpr::Concat(items) => items.iter().map(|item| expr_width(item, widths)).sum(),
    }
}

/// 取向量化 assign 的第 `index` 位；标量与常量按 Verilog 规则零扩展。
pub(super) fn bit_expr(
    expr: &VExpr,
    index: usize,
    widths: &BTreeMap<String, usize>,
    ascending: &BTreeSet<String>,
) -> Result<VExpr, VerilogError> {
    match expr {
        VExpr::Var(name) => {
            let width = widths.get(name).copied().unwrap_or(1);
            if width > 1 {
                if index >= width {
                    return Err(message(format!(
                        "位 {index} 超出向量 {name} 的宽度 {width}"
                    )));
                }
                // Verilog 左界是 MSB：升序 `[0:N]` 的位置 `index`（0=LSB）
                // 对应声明下标 `width-1-index`。
                let declared = if ascending.contains(name) {
                    width - 1 - index
                } else {
                    index
                };
                Ok(VExpr::Var(format!("{name}[{declared}]")))
            } else if index == 0 {
                Ok(VExpr::Var(name.clone()))
            } else {
                Ok(VExpr::Const(Ternary::False))
            }
        }
        VExpr::Const(value) => Ok(VExpr::Const(if index == 0 {
            *value
        } else {
            Ternary::False
        })),
        VExpr::ConstBits { bits, .. } => Ok(VExpr::Const(
            bits.get(index).copied().unwrap_or(Ternary::False),
        )),
        VExpr::Not(inner) => Ok(VExpr::Not(Box::new(bit_expr(
            inner, index, widths, ascending,
        )?))),
        VExpr::And(a, b) => Ok(VExpr::And(
            Box::new(bit_expr(a, index, widths, ascending)?),
            Box::new(bit_expr(b, index, widths, ascending)?),
        )),
        VExpr::Or(a, b) => Ok(VExpr::Or(
            Box::new(bit_expr(a, index, widths, ascending)?),
            Box::new(bit_expr(b, index, widths, ascending)?),
        )),
        VExpr::Xor(a, b) => Ok(VExpr::Xor(
            Box::new(bit_expr(a, index, widths, ascending)?),
            Box::new(bit_expr(b, index, widths, ascending)?),
        )),
        VExpr::Concat(items) => {
            let mut offset = 0usize;
            for item in items.iter().rev() {
                let width = expr_width(item, widths);
                if index < offset + width {
                    return bit_expr(item, index - offset, widths, ascending);
                }
                offset += width;
            }
            Err(message(format!("拼接位 {index} 越界")))
        }
    }
}
