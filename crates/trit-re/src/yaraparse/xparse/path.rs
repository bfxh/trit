//! 标识符与模块对象路径：循环变量、`pe.<字段>`、`[下标]`、`["键"]`、
//! `函数(实参)`。静态类型检查照官方：
//! - 没 `import "pe"` 就用 `pe.` / 用了没声明的名字 = undefined identifier；
//! - 结构里没有的字段 = invalid field name；
//! - 数组下标必须是整数、字典键必须是字符串；
//! - 函数实参签名对不上任何重载 = wrong arguments；
//! - 结构 / 数组 / 函数本身不能当值（wrong usage of identifier）。
//!
//! `pe.<全大写常量>` 直接落成整数字面量节点，但**不算编译期常量**（官方
//! 的模块常量在扫描期才赋值——`1 \ (pe.MACHINE_I386 - 332)` 能通过编译）。

use super::boolean::{Parser, Res, Ty, Typed, compile_regex, show};
use super::lex::T;
use super::sets::value_type;
use crate::sigscan::expr::schema::{PE, Sch, constant};
use crate::sigscan::expr::{Arg, Expr, Path, Root, Seg};

static INT: Sch = Sch::Int;
static STR: Sch = Sch::Str;

impl Parser<'_> {
    /// 值位置上的标识符（`primary_expression : identifier`）。
    pub(super) fn identifier(&mut self, word: &str) -> Res<Typed> {
        if word == "pe"
            && self.has_pe
            && let (Some(T::P(".")), Some(T::Ident(name))) = (self.peek(), self.peek_at(1))
            && let Some(value) = constant(name)
        {
            self.pos += 2;
            return self.node(Expr::Int(value), Ty::Int, true, &[]);
        }
        let (path, sch, depth) = self.object(word)?;
        let ty = match value_type(sch) {
            Ty::Obj(_) => {
                return Err(format!(
                    "`{word}…` 是结构 / 数组 / 函数，不能直接当值用（官方：wrong usage of identifier）"
                ));
            }
            scalar => scalar,
        };
        let expr = match (path.root, path.segs.is_empty()) {
            (Root::Var(slot), true) => Expr::Var(slot),
            _ => Expr::Path(path),
        };
        self.node(expr, ty, true, &[depth])
    }

    /// 走完一条路径 → `(路径, 末端类型, 路径里表达式的最大深度)`。
    pub(super) fn object(&mut self, word: &str) -> Res<(Path, &'static Sch, u32)> {
        let (root, mut sch) = self.root(word)?;
        // 路径段各自一个函数：`pe.a[pe.b[…]]` 每深一层都要再压一遍 `object`
        // 的栈帧，段的细节不摊在这里（栈帧说明见 `primary.rs`）。
        let (mut segs, mut depth) = (Vec::new(), 0u32);
        loop {
            let deepest;
            (sch, deepest) = if self.eat_p(".") {
                (self.field(sch, &mut segs)?, 0)
            } else if self.eat_p("[") {
                self.subscript(sch, &mut segs)?
            } else if self.eat_p("(") {
                self.call(sch, &mut segs)?
            } else {
                return Ok((Path { root, segs }, sch, depth));
            };
            depth = depth.max(deepest);
        }
    }

    /// 路径的根：循环变量或 `pe` 模块。
    fn root(&self, word: &str) -> Res<(Root, &'static Sch)> {
        let slot = self.scope.iter().position(|(name, _)| name == word);
        Ok(match slot {
            Some(slot) => match self.scope[slot].1 {
                Ty::Int => (Root::Var(slot), &INT),
                Ty::Str => (Root::Var(slot), &STR),
                Ty::Obj(sch) => (Root::Var(slot), sch),
                Ty::Bool => return Err(format!("循环变量 `{word}` 不是值")),
            },
            None if word == "pe" && self.has_pe => (Root::Pe, &PE),
            None if word == "pe" => {
                return Err(
                    "用了 `pe.` 却没有 `import \"pe\"`（官方：undefined identifier）".into(),
                );
            }
            None => {
                return Err(format!(
                    "未定义的标识符 `{word}`（规则引用与 pe 以外的模块不在子集内；官方未声明时：undefined identifier）"
                ));
            }
        })
    }

    fn field(&mut self, sch: &'static Sch, segs: &mut Vec<Seg>) -> Res<&'static Sch> {
        let Some(T::Ident(name)) = self.peek() else {
            return Err(format!("`.` 之后应为字段名，实际是{}", show(self.peek())));
        };
        self.pos += 1;
        let Some((name, next)) = sch.field(name) else {
            return Err(format!(
                "`{name}` 不是这里的合法字段（官方：invalid field name）"
            ));
        };
        segs.push(Seg::Field(name));
        Ok(next)
    }

    fn subscript(&mut self, sch: &'static Sch, segs: &mut Vec<Seg>) -> Res<(&'static Sch, u32)> {
        let at = self.boolean()?;
        self.expect_p("]")?;
        match sch {
            Sch::Array(item) => {
                self.want_int(&at, "数组下标")?;
                segs.push(Seg::Index(at.expr));
                Ok((item, at.depth))
            }
            Sch::Dict(item) => {
                self.want_str(&at, "字典键")?;
                segs.push(Seg::Key(at.expr));
                Ok((item, at.depth))
            }
            _ => Err("`[…]` 只能用在数组或字典上（官方：wrong type）".into()),
        }
    }

    fn call(&mut self, sch: &'static Sch, segs: &mut Vec<Seg>) -> Res<(&'static Sch, u32)> {
        let Sch::Func(overloads) = sch else {
            return Err("`(…)` 只能用在函数上（官方：wrong type）".into());
        };
        let (mut args, mut signature, mut depth) = (Vec::new(), String::new(), 0u32);
        if !self.eat_p(")") {
            loop {
                depth = depth.max(self.argument(&mut args, &mut signature)?);
                if !self.eat_p(",") {
                    break;
                }
            }
            self.expect_p(")")?;
        }
        let Some((_, returns_str)) = overloads.iter().find(|(sig, _)| *sig == signature) else {
            return Err(format!(
                "函数实参签名 ({signature}) 对不上任何重载（官方：wrong arguments）"
            ));
        };
        segs.push(Seg::Call(args));
        Ok((if *returns_str { &STR } else { &INT }, depth))
    }

    /// 一个实参 → 它的深度。签名字符：`i` 整数、`s` 字符串、`r` 正则、
    /// `b` 布尔（pe 没有收布尔的函数）。
    fn argument(&mut self, args: &mut Vec<Arg>, signature: &mut String) -> Res<u32> {
        if let Some(T::Regex { body, flags }) = self.peek() {
            self.pos += 1;
            args.push(Arg::Re(compile_regex(body, flags)?));
            signature.push('r');
            return Ok(0);
        }
        let value = self.boolean()?;
        signature.push(match value.ty {
            Ty::Int => 'i',
            Ty::Str => 's',
            Ty::Bool => 'b',
            Ty::Obj(_) => return Err("函数实参不能是结构 / 数组 / 函数".into()),
        });
        args.push(Arg::Val(*value.expr));
        Ok(value.depth)
    }
}
