//! 数值叶子与括号跳过（从 `cond.rs` 拆出——模块预算棘轮纪律：拆文件
//! 不抬台账）。
//!
//! 数值叶子：字面量、`filesize`/`entrypoint`、`uintN(lit)`、`#串`/`@串`、
//! 模块路径（含纯数值数组下标 `pe.sections[0].characteristics`）。其余词首
//! 如实报错——编译错误的 YARA 不该被装作能评估。

use super::Tok;
use super::cond::CondParser;
use crate::sigscan::Operand;

impl CondParser<'_> {
    /// 数值叶子：字面量（十进制/`0x`/`KB`·`MB`）、`filesize`、`entrypoint`、
    /// `uintN(lit)`、`#串`/`#$` 计数、`@串[N]`/`@$[N]` 偏移、模块引用与
    /// 未知函数（记名 → 求值恒 Unknown）。其余词首如实报错。
    pub(super) fn parse_num_leaf(&mut self) -> Result<Operand, String> {
        let Some(Tok::Word(w)) = self.peek().cloned() else {
            return self.bail("缺数值");
        };
        if let Some(v) = super::cond::parse_size(&w) {
            self.pos += 1;
            return Ok(Operand::Lit(v));
        }
        match w.as_str() {
            "filesize" => {
                self.pos += 1;
                return Ok(Operand::Filesize);
            }
            "entrypoint" => {
                self.pos += 1;
                return Ok(Operand::Entrypoint);
            }
            "uint8" | "uint16" | "uint32" => {
                let size = match w.as_str() {
                    "uint8" => 1u8,
                    "uint16" => 2,
                    _ => 4,
                };
                self.pos += 1;
                if !self.eat_sym('(') {
                    return self.bail("uintN 后缺 (");
                }
                let offset = self.parse_int()?;
                if !self.eat_sym(')') {
                    return self.bail("uintN 未闭合");
                }
                return Ok(Operand::UintRead { size, offset });
            }
            _ => {}
        }
        if let Some(rest) = w.strip_prefix('#') {
            self.pos += 1;
            return self.count_operand(rest);
        }
        if let Some(rest) = w.strip_prefix('@') {
            self.pos += 1;
            return self.offset_operand(rest);
        }
        // 模块路径 `pe.xxx`（含数组式下标 `pe.sections[0].characteristics`）
        // 与未实现函数 `math.entropy(…)`：记名弃权。裸未知词（不带点不带
        // 下标不带调用）如实报错——不装作能评估。
        let next_is = |t: &Tok| self.toks.get(self.pos + 1) == Some(t);
        if next_is(&Tok::Sym('.')) || next_is(&Tok::Sym('(')) || next_is(&Tok::Sym('[')) {
            self.pos += 1; // 吃掉词本身
            let mut path = w;
            loop {
                if self.eat_sym('.') {
                    match self.next() {
                        Some(Tok::Word(seg)) => {
                            path.push('.');
                            path.push_str(&seg);
                        }
                        other => return self.bail(&format!("模块路径缺段：{other:?}")),
                    }
                } else if self.eat_sym('[') {
                    // 数组式下标：纯数值 = 捕获索引进路径（pe.sections[0]
                    // 实装）；其余吞平衡 [ ]（求值层记名弃权），后续段继续
                    if let Some(Tok::Word(d)) = self.peek()
                        && !d.is_empty()
                        && d.bytes().all(|b| b.is_ascii_digit())
                    {
                        let digits = d.clone();
                        self.pos += 1; // 数字词
                        if !self.eat_sym(']') {
                            return self.bail("模块路径下标未闭合");
                        }
                        path.push('[');
                        path.push_str(&digits);
                        path.push(']');
                        continue;
                    }
                    let mut depth = 1i32;
                    while let Some(t) = self.next() {
                        match t {
                            Tok::Sym('[') => depth += 1,
                            Tok::Sym(']') => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    if depth != 0 {
                        return self.bail("模块路径下标未闭合");
                    }
                    path.push_str("[…]");
                } else if self.eat_sym('(') {
                    self.skip_balanced()?;
                    path.push_str("(…)");
                    break;
                } else {
                    break;
                }
            }
            return Ok(Operand::Opaque(path));
        }
        self.bail("无法识别的数值起点")
    }

    /// 吃掉一段平衡的括号（调用方已吃 `(`）——未实现函数的参数原样跳过。
    fn skip_balanced(&mut self) -> Result<(), String> {
        let mut depth = 1i32;
        while let Some(t) = self.next() {
            match t {
                Tok::Sym('(') => depth += 1,
                Tok::Sym(')') => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                _ => {}
            }
        }
        Err("条件不支持：括号未闭合".into())
    }
}
