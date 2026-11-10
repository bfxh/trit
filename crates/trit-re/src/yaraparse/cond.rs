//! 条件表达式递归下降（从 `yaraparse.rs` 拆出，控制单文件行数预算）：
//! 布尔层（and/or/not）→ of/for-of 集合量词 → 数值叶子与比较。
//! 解析不了的语法如实 `Err`（→ 整条规则 `Unsupported`），绝不静默吞。

use super::Tok;
use crate::sigscan::{CmpOp, Condition, ForBound, Operand, Pattern};

pub(super) struct CondParser<'a> {
    toks: &'a [Tok],
    pos: usize,
    strings: &'a [(String, Pattern)],
    /// for-of 嵌套深度：`$`/`#$`/`@$` 只在体内合法（>0），体外如实报错。
    for_depth: usize,
}

impl<'a> CondParser<'a> {
    pub(super) fn new(toks: &'a [Tok], strings: &'a [(String, Pattern)]) -> Self {
        Self {
            toks,
            pos: 0,
            strings,
            for_depth: 0,
        }
    }

    /// 解析整个条件并检查 token 全部消化（残留 = 语法不支持，如实报错）。
    pub(super) fn parse_full(&mut self) -> Result<Condition, String> {
        let cond = self.parse_expr()?;
        if self.pos != self.toks.len() {
            return Err(format!(
                "条件尾部有未消化的 token：{:?}",
                self.toks.get(self.pos)
            ));
        }
        Ok(cond)
    }

    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn eat_sym(&mut self, c: char) -> bool {
        if self.peek() == Some(&Tok::Sym(c)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn eat_word(&mut self, w: &str) -> bool {
        if self.word_is(w) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn word_is(&self, w: &str) -> bool {
        self.peek() == Some(&Tok::Word(w.to_string()))
    }

    fn bail<T>(&self, why: &str) -> Result<T, String> {
        Err(format!("条件不支持：{why}（token {:?}）", self.peek()))
    }

    /// `or` 优先级最低。
    fn parse_expr(&mut self) -> Result<Condition, String> {
        let mut left = self.parse_and()?;
        while self.word_is("or") {
            self.pos += 1;
            let right = self.parse_and()?;
            left = match left {
                Condition::Or(mut v) => {
                    v.push(right);
                    Condition::Or(v)
                }
                other => Condition::Or(vec![other, right]),
            };
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Condition, String> {
        let mut left = self.parse_unit()?;
        while self.word_is("and") {
            self.pos += 1;
            let right = self.parse_unit()?;
            left = match left {
                Condition::And(mut v) => {
                    v.push(right);
                    Condition::And(v)
                }
                other => Condition::And(vec![other, right]),
            };
        }
        Ok(left)
    }

    fn parse_unit(&mut self) -> Result<Condition, String> {
        if self.word_is("not") {
            self.pos += 1;
            let inner = self.parse_unit()?;
            return Ok(Condition::Not(Box::new(inner)));
        }
        if self.eat_word("any") {
            if !self.eat_word("of") {
                return self.bail("any 后缺 of");
            }
            return self.parse_of_tail(true, None);
        }
        if self.eat_word("all") {
            if !self.eat_word("of") {
                return self.bail("all 后缺 of");
            }
            return self.parse_of_tail(false, None);
        }
        if self.eat_word("for") {
            return self.parse_for();
        }
        if self.eat_word("true") {
            return Ok(Condition::Const(true));
        }
        if self.eat_word("false") {
            return Ok(Condition::Const(false));
        }
        if self.eat_sym('(') {
            let inner = self.parse_expr()?;
            if !self.eat_sym(')') {
                return self.bail("括号未闭合");
            }
            return Ok(inner);
        }
        // N of … / $ref … / 数值叶子 + 比较
        if let Some(Tok::Word(w)) = self.peek().cloned() {
            if let Ok(n) = w.parse::<usize>() {
                self.pos += 1;
                if self.eat_word("of") {
                    return self.parse_of_tail(false, Some(n));
                }
                return self.cmp_tail(Operand::Lit(n as u64));
            }
            // `$`（for-of 体内的当前串）/ `$id` 串引用
            if w == "$" {
                self.pos += 1;
                if self.for_depth == 0 {
                    return self.bail("'$' 只能在 for-of 体内使用");
                }
                if self.eat_word("at") {
                    return Ok(Condition::CurrentAt {
                        offset: self.parse_num_leaf()?,
                    });
                }
                if self.eat_word("in") {
                    return self.parse_in_tail_current();
                }
                return Ok(Condition::CurrentRef);
            }
            if let Some(id) = w.strip_prefix('$') {
                self.pos += 1;
                return self.parse_ref_tail(id);
            }
            // 其余词首：数值叶子（filesize/uintN/#/@/模块/带后缀数字）+ 比较
            let lhs = self.parse_num_leaf()?;
            return self.cmp_tail(lhs);
        }
        self.bail("无法识别的条件起点")
    }

    /// `for <N|any|all> of <them|($x*)> : ( 体 )`——体内 `$`/`#$`/`@$`
    /// 绑定当前迭代串。
    fn parse_for(&mut self) -> Result<Condition, String> {
        let bound = if self.eat_word("any") {
            ForBound::Any
        } else if self.eat_word("all") {
            ForBound::All
        } else {
            ForBound::N(self.parse_int()? as usize)
        };
        if !self.eat_word("of") {
            return self.bail("for 后缺 of");
        }
        let prefix = self.parse_string_set()?;
        if !self.eat_sym(':') {
            return self.bail("for-of 缺 ':'");
        }
        if !self.eat_sym('(') {
            return self.bail("for-of 体缺 '('");
        }
        self.for_depth += 1;
        let body = self.parse_expr();
        self.for_depth -= 1;
        let body = body?;
        if !self.eat_sym(')') {
            return self.bail("for-of 体未闭合");
        }
        Ok(Condition::ForOf {
            bound,
            prefix,
            body: Box::new(body),
        })
    }

    /// `of` 之后的串集合：`them` → `None`，`($x*)` → 前缀。
    fn parse_string_set(&mut self) -> Result<Option<String>, String> {
        if self.eat_word("them") {
            return Ok(None);
        }
        if !self.eat_sym('(') {
            return self.bail("缺 them 或 (");
        }
        let Some(Tok::Word(w)) = self.next() else {
            return self.bail("缺串引用");
        };
        let Some(id) = w.strip_prefix('$') else {
            return self.bail("缺 $ 引用");
        };
        let prefix = id.strip_suffix('*').map(|p| p.to_string());
        if !self.eat_sym(')') {
            return self.bail("串集合未闭合");
        }
        Ok(prefix)
    }

    /// `of` 之后的尾巴（调用方已消费 `of`）：
    /// `any` → OfAny、`all` → OfAll、`N` → OfCount。
    fn parse_of_tail(&mut self, any: bool, n: Option<usize>) -> Result<Condition, String> {
        let prefix = self.parse_string_set()?;
        Ok(match (any, n) {
            (true, _) => Condition::OfAny { prefix },
            (false, None) => Condition::OfAll { prefix },
            (false, Some(n)) => Condition::OfCount { n, prefix },
        })
    }

    /// 数值叶子之后的比较尾巴：`<cmpop> <数值叶子>`。裸数值不是布尔
    ///（`filesize` 单独出现 = 条件不支持，如实报错）。
    fn cmp_tail(&mut self, lhs: Operand) -> Result<Condition, String> {
        let Some(op) = self.parse_cmp_op() else {
            return self.bail("数值后缺比较算子（裸数值不是布尔条件）");
        };
        let rhs = self.parse_num_leaf()?;
        Ok(Condition::Cmp { lhs, op, rhs })
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

    /// 数值叶子：字面量（十进制/`0x`/`KB`·`MB`）、`filesize`、`entrypoint`、
    /// `uintN(lit)`、`#串`/`#$` 计数、`@串[N]`/`@$[N]` 偏移、模块引用与
    /// 未知函数（记名 → 求值恒 Unknown）。其余词首如实报错——编译错误的
    /// YARA 不该被装作能评估。
    fn parse_num_leaf(&mut self) -> Result<Operand, String> {
        let Some(Tok::Word(w)) = self.peek().cloned() else {
            return self.bail("缺数值");
        };
        if let Some(v) = parse_size(&w) {
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
        // 模块路径 `pe.xxx` 与未实现函数 `math.entropy(…)`：记名弃权。
        // 裸未知词（不带点不带调用）如实报错——不装作能评估。
        let next_is = |t: &Tok| self.toks.get(self.pos + 1) == Some(t);
        if next_is(&Tok::Sym('.')) || next_is(&Tok::Sym('(')) {
            self.pos += 1; // 吃掉词本身
            let mut path = w;
            while self.eat_sym('.') {
                match self.next() {
                    Some(Tok::Word(seg)) => {
                        path.push('.');
                        path.push_str(&seg);
                    }
                    other => return self.bail(&format!("模块路径缺段：{other:?}")),
                }
            }
            if self.eat_sym('(') {
                self.skip_balanced()?;
                path.push_str("(…)");
            }
            return Ok(Operand::Opaque(path));
        }
        self.bail("无法识别的数值起点")
    }

    /// `#` 后的串名：`#a`（计数）/ `#$` / 裸 `#`（for-of 体内当前串）。
    fn count_operand(&mut self, rest: &str) -> Result<Operand, String> {
        if rest.is_empty() || rest == "$" {
            if self.for_depth == 0 {
                return self.bail("'#' 只能在 for-of 体内使用");
            }
            return Ok(Operand::CurrentCount);
        }
        let index = self.string_index(rest)?;
        Ok(Operand::Count(index))
    }

    /// `@` 后的串名与可选 `[N]`：`@a` / `@a[2]` / `@$` / 裸 `@$[2]`。
    /// **下标 1 起**（官方 YARA 语义，yara-python 实测判定：`@a[1]` =
    /// 首次、`@a[0]` 无效）——解析成 0 起的内部下标。
    fn offset_operand(&mut self, rest: &str) -> Result<Operand, String> {
        let current = rest.is_empty() || rest == "$";
        let mut nth = 0usize;
        if self.eat_sym('[') {
            let n = self.parse_int()? as usize;
            if n == 0 {
                return self.bail("@ 下标从 1 起（官方 YARA 语义）");
            }
            nth = n - 1;
            if !self.eat_sym(']') {
                return self.bail("@ 偏移下标未闭合");
            }
        }
        if current {
            if self.for_depth == 0 {
                return self.bail("'@' 只能在 for-of 体内使用");
            }
            return Ok(Operand::CurrentOffset { nth });
        }
        let index = self.string_index(rest)?;
        Ok(Operand::Offset { index, nth })
    }

    /// for-of 体内 `$ in (a..b)`（调用方已消费 `in`）。
    fn parse_in_tail_current(&mut self) -> Result<Condition, String> {
        if !self.eat_sym('(') {
            return self.bail("in 后缺 (");
        }
        let lo = self.parse_int()?;
        let sep_ok = if self.peek() == Some(&Tok::Sym('.')) {
            self.pos += 1;
            self.eat_sym('.')
        } else if self.word_is("..") {
            self.pos += 1;
            true
        } else {
            false
        };
        if !sep_ok {
            return self.bail("in 区间缺 ..");
        }
        let hi = self.parse_int()?;
        if !self.eat_sym(')') {
            return self.bail("in 区间未闭合");
        }
        Ok(Condition::CurrentIn { lo, hi })
    }

    fn string_index(&self, id: &str) -> Result<usize, String> {
        self.strings
            .iter()
            .position(|(sid, _)| sid == id)
            .ok_or_else(|| format!("条件不支持：引用了未声明的串 ${id}"))
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

    /// `$id` 尾巴：裸引用 / `at <数值叶子>` / `in (a .. b)`。
    fn parse_ref_tail(&mut self, id: &str) -> Result<Condition, String> {
        let index = match self.strings.iter().position(|(sid, _)| sid == id) {
            Some(i) => i,
            None => return self.bail(&format!("引用了未声明的串 ${id}")),
        };
        if self.eat_word("at") {
            let offset = self.parse_num_leaf()?;
            return Ok(Condition::At { index, offset });
        }
        if self.eat_word("in") {
            if !self.eat_sym('(') {
                return self.bail("in 后缺 (");
            }
            let lo = self.parse_int()?;
            // 区间分隔符：`..` 在词法层被 '.' 断字拆成两个 Sym('.')，
            // 带空格的 `..` 也可能保持 Word 形态——两种都认。
            let sep_ok = if self.peek() == Some(&Tok::Sym('.')) {
                self.pos += 1;
                self.eat_sym('.')
            } else if self.word_is("..") {
                self.pos += 1;
                true
            } else {
                false
            };
            if !sep_ok {
                return self.bail("in 区间缺 ..");
            }
            let hi = self.parse_int()?;
            if !self.eat_sym(')') {
                return self.bail("in 区间未闭合");
            }
            return Ok(Condition::In { index, lo, hi });
        }
        Ok(Condition::Ref(index))
    }

    /// 整数字面量（十进制 / 0x 十六进制 / KB·MB 后缀）。
    /// 算术表达式不支持（fail-closed）。
    fn parse_int(&mut self) -> Result<u64, String> {
        let Some(Tok::Word(w)) = self.next() else {
            return self.bail("缺整数");
        };
        parse_size(&w).ok_or_else(|| format!("条件不支持：非法整数 {w:?}"))
    }
}

/// 数值字面量：十进制 / `0x` 十六进制 / `KB`·`MB` 后缀（YARA 尺寸单位，
/// 大小写不敏感）。其余 → `None`。
pub(super) fn parse_size(tok: &str) -> Option<u64> {
    let (num, mul) = if let Some(n) = tok.strip_suffix("KB").or_else(|| tok.strip_suffix("kb")) {
        (n, 1024u64)
    } else if let Some(n) = tok.strip_suffix("MB").or_else(|| tok.strip_suffix("mb")) {
        (n, 1024 * 1024)
    } else {
        (tok, 1)
    };
    let v = if let Some(hex) = num.strip_prefix("0x") {
        u64::from_str_radix(hex, 16).ok()?
    } else {
        num.parse::<u64>().ok()?
    };
    v.checked_mul(mul)
}
