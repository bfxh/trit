//! 比较尾巴解析（从 `cond.rs` 拆出守预算）：数值比较 + 字符串等值比较
//! （`pe.sections[N].name == ".text"` 等）。

use super::Tok;
use super::cond::CondParser;
use crate::sigscan::{CmpOp, Condition, Operand};

impl CondParser<'_> {
    /// 数值叶子之后的比较尾巴：`<cmpop> <数值叶子>`。裸数值不是布尔
    ///（`filesize` 单独出现 = 条件不支持，如实报错）——例外：官方
    /// 允许裸布尔函数（`condition: pe.is_dll()`；探针实测 is_dll /
    /// is_32bit 都是**函数**要 `()`，裸属性 `pe.is_dll` 官方编译报错），
    /// 已知布尔函数按真值语义收窄为 `!= 0`；其余裸路径仍如实报错。
    pub(super) fn cmp_tail(&mut self, lhs: Operand) -> Result<Condition, String> {
        if let Some(op) = self.parse_cmp_op() {
            // 右侧字符串字面量 → 字符串比较（官方只支持 == / !=）
            if let Some(Tok::Str(s)) = self.peek().cloned() {
                self.pos += 1;
                if !matches!(op, CmpOp::Eq | CmpOp::Ne) {
                    return self.bail("字符串只支持 == / !=");
                }
                if !is_string_operand(&lhs) {
                    return self.bail("字符串比较的另一侧必须是字符串属性");
                }
                return Ok(Condition::Cmp {
                    lhs,
                    op,
                    rhs: Operand::LitStr(s),
                });
            }
            if matches!(lhs, Operand::LitStr(_)) {
                return self.bail("字符串只能与字符串比较");
            }
            let rhs = self.parse_num_leaf()?;
            return Ok(Condition::Cmp { lhs, op, rhs });
        }
        if matches!(
            &lhs,
            Operand::Opaque(p) if p == "pe.is_dll(…)" || p == "pe.is_32bit(…)"
        ) {
            return Ok(Condition::Cmp {
                lhs,
                op: CmpOp::Ne,
                rhs: Operand::Lit(0),
            });
        }
        self.bail("数值后缺比较算子（裸数值不是布尔条件）")
    }

    /// 比较算子：`== != < <= > >=`（不匹配时不消费任何 token）。
    fn parse_cmp_op(&mut self) -> Option<CmpOp> {
        match self.peek() {
            Some(Tok::Sym('=')) => {
                self.pos += 1;
                if self.eat_sym('=') {
                    Some(CmpOp::Eq)
                } else {
                    self.pos -= 1;
                    None
                }
            }
            Some(Tok::Sym('!')) => {
                self.pos += 1;
                if self.eat_sym('=') {
                    Some(CmpOp::Ne)
                } else {
                    self.pos -= 1;
                    None
                }
            }
            Some(Tok::Sym('<')) => {
                self.pos += 1;
                if self.eat_sym('=') {
                    Some(CmpOp::Le)
                } else {
                    Some(CmpOp::Lt)
                }
            }
            Some(Tok::Sym('>')) => {
                self.pos += 1;
                if self.eat_sym('=') {
                    Some(CmpOp::Ge)
                } else {
                    Some(CmpOp::Gt)
                }
            }
            _ => None,
        }
    }
}

/// 解析层字符串属性白名单（求值层 `sigscan::strval` 对应同集合）。
fn is_string_operand(op: &Operand) -> bool {
    match op {
        Operand::LitStr(_) => true,
        Operand::Opaque(p) => {
            (p.starts_with("pe.sections[") && p.ends_with(".name"))
                || p == "pe.rich_signature.clear_data"
                || p == "pe.rich_signature.raw_data"
        }
        _ => false,
    }
}
