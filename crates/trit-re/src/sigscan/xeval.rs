//! 表达式求值（接缝实现）：`XCtx` / `Env` / `eval`。
//!
//! 三态纪律见 `docs/YARA-EXPR/01-SEMANTICS.md`：`Unknown` 是证据缺失，
//! 绝不折叠成假；`Undef` 是官方 undefined，到裁决才当假。

use super::expr::{Expr, Iter, ObjPath, OfSet, OfTail, Quant, Root, Seg, StrRef};
use super::modreg::{self, Node};
use super::xval::{Val, bin, bit_not, cmp, defined, int, neg, not, to_int, truth};
use super::{pe2, xfacts::StrFact, xloop, xstr};

/// 每条规则的循环体求值总次数上限。
pub const EVAL_BUDGET: u64 = 1 << 22;

/// 求值上下文（`strings`/`imports` 供 y2 前端接入后使用，P1）。
#[allow(dead_code)]
pub struct XCtx<'a> {
    pub data: &'a [u8],
    pub truncated: bool,
    pub entrypoint: Option<u64>,
    pub tree: &'a Node,
    pub facts: &'a [StrFact],
    pub strings: &'a [String],
    pub rules: &'a [(String, Val)],
    pub imports: &'a [String],
}

/// 循环变量槽位（第 L 层第一个变量 = 2L，第二个 = 2L+1）。
#[derive(Clone)]
pub struct Env {
    pub slots: [Val; 8],
    pub objects: [Option<Node>; 8],
    pub current: Option<usize>,
    pub budget: u64,
}

impl Default for Env {
    fn default() -> Self {
        Self {
            slots: std::array::from_fn(|_| Val::Unknown),
            objects: std::array::from_fn(|_| None),
            current: None,
            budget: EVAL_BUDGET,
        }
    }
}

fn str_index(r: &StrRef, env: &Env) -> Option<usize> {
    match r {
        StrRef::Id(i) => Some(*i),
        StrRef::Current => env.current,
    }
}

fn fact<'a>(ctx: &'a XCtx, r: &StrRef, env: &Env) -> Option<&'a StrFact> {
    let i = str_index(r, env)?;
    ctx.facts.get(i)
}

fn quant_val(q: &Quant, ctx: &XCtx, env: &mut Env) -> Option<xloop::QuantVal> {
    Some(match q {
        Quant::All => xloop::QuantVal::All,
        Quant::Any => xloop::QuantVal::Any,
        Quant::None => xloop::QuantVal::None,
        Quant::N(e) => match eval(e, ctx, env) {
            Val::Int(n) => xloop::QuantVal::N(n),
            // 运行期 undefined 按 all 处理（官方读数 M103）。
            Val::Undef => xloop::QuantVal::All,
            _ => return None,
        },
        Quant::Percent(e) => match eval(e, ctx, env) {
            Val::Int(n) => xloop::QuantVal::Percent(n as f64),
            Val::Undef => xloop::QuantVal::Percent(0.0),
            _ => return None,
        },
    })
}

fn count3(v: Val, t: &mut i64, f: &mut i64, u: &mut i64) {
    match truth(&v) {
        Some(true) => *t += 1,
        Some(false) => *f += 1,
        None => *u += 1,
    }
}

fn dotall(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut esc = false;
    for c in src.chars() {
        if esc {
            out.push(c);
            esc = false;
        } else if c == '\\' {
            out.push(c);
            esc = true;
        } else if c == '.' {
            out.push_str("[\\s\\S]");
        } else {
            out.push(c);
        }
    }
    out
}

pub fn eval(expr: &Expr, ctx: &XCtx, env: &mut Env) -> Val {
    env.budget = env.budget.saturating_sub(1);
    if env.budget == 0 {
        return Val::Unknown;
    }
    match expr {
        Expr::Int(n) => int(*n),
        Expr::Float(bits) => Val::Float(*bits),
        Expr::Str(s) => Val::Str(s.clone()),
        Expr::Bool(b) => Val::Int(i64::from(*b)),
        Expr::Regex(r) => Val::Regex(r.clone()),
        Expr::Filesize => {
            if ctx.truncated {
                Val::Unknown
            } else {
                int(ctx.data.len() as i64)
            }
        }
        Expr::Entrypoint => ctx.entrypoint.map_or(Val::Unknown, |v| int(v as i64)),
        Expr::ReadInt {
            signed,
            bytes,
            big_endian,
            at,
        } => read_int(*signed, *bytes, *big_endian, eval(at, ctx, env), ctx),
        Expr::Found(r) => fact(ctx, r, env).map_or(Val::Unknown, StrFact::found),
        Expr::FoundAt(r, at) => {
            let at = eval(at, ctx, env);
            fact(ctx, r, env).map_or(Val::Unknown, |f| f.found_at(at))
        }
        Expr::FoundIn(r, lo, hi) => {
            let lo = eval(lo, ctx, env);
            let hi = eval(hi, ctx, env);
            fact(ctx, r, env).map_or(Val::Unknown, |f| f.found_in(lo, hi))
        }
        Expr::Count(r) => fact(ctx, r, env).map_or(Val::Unknown, StrFact::count),
        Expr::CountIn(r, lo, hi) => {
            let lo = eval(lo, ctx, env);
            let hi = eval(hi, ctx, env);
            fact(ctx, r, env).map_or(Val::Unknown, |f| f.count_in(lo, hi))
        }
        Expr::Offset(r, nth) => {
            let nth = eval(nth, ctx, env);
            fact(ctx, r, env).map_or(Val::Unknown, |f| f.offset(nth))
        }
        Expr::Length(r, nth) => {
            let nth = eval(nth, ctx, env);
            fact(ctx, r, env).map_or(Val::Unknown, |f| f.length(nth))
        }
        Expr::Var(slot) => env
            .slots
            .get(*slot as usize)
            .cloned()
            .unwrap_or(Val::Unknown),
        Expr::Obj(path) => eval_obj(path, ctx, env),
        Expr::Neg { float, e } => {
            let v = eval(e, ctx, env);
            neg(*float, v)
        }
        Expr::BitNot(e) => bit_not(eval(e, ctx, env)),
        Expr::Bin { op, float, a, b } => {
            let a = eval(a, ctx, env);
            let b = eval(b, ctx, env);
            bin(*op, *float, a, b)
        }
        Expr::Cmp { op, kind, a, b } => {
            let a = eval(a, ctx, env);
            let b = eval(b, ctx, env);
            cmp(*op, *kind, a, b)
        }
        Expr::StrOp(op, a, b) => {
            let a = eval(a, ctx, env);
            let b = eval(b, ctx, env);
            match (a, b) {
                (Val::Str(x), Val::Str(y)) => Val::Int(i64::from(xstr::apply(*op, &x, &y))),
                (Val::Undef, _) | (_, Val::Undef) => Val::Undef,
                (Val::Unknown, _) | (_, Val::Unknown) => Val::Unknown,
                _ => Val::Undef,
            }
        }
        Expr::Matches(a, re) => {
            let a = eval(a, ctx, env);
            let Val::Str(hay) = a else {
                return match a {
                    Val::Undef => Val::Undef,
                    _ => Val::Unknown,
                };
            };
            let src = if re.dotall {
                dotall(&re.src)
            } else {
                re.src.clone()
            };
            match super::regex::compile(&src, re.nocase) {
                Ok(r) => {
                    let (hits, _) = super::regex::find_all(&hay, &r, super::SCAN_BUDGET);
                    Val::Int(i64::from(!hits.is_empty()))
                }
                Err(_) => Val::Unknown,
            }
        }
        Expr::Not(e) => {
            let v = eval(e, ctx, env);
            not(v)
        }
        Expr::Defined(e) => {
            let v = eval(e, ctx, env);
            defined(v)
        }
        Expr::And(a, b) => {
            let x = eval(a, ctx, env);
            let y = eval(b, ctx, env);
            match (truth(&x), truth(&y)) {
                (Some(false), _) | (_, Some(false)) => Val::Int(0),
                (Some(true), Some(true)) => Val::Int(1),
                _ => Val::Unknown,
            }
        }
        Expr::Or(a, b) => {
            let x = eval(a, ctx, env);
            let y = eval(b, ctx, env);
            match (truth(&x), truth(&y)) {
                (Some(true), _) => x,
                (_, Some(true)) => Val::Int(1),
                (Some(false), Some(false)) => Val::Int(0),
                _ => Val::Unknown,
            }
        }
        Expr::StrToBool(e) => to_int(eval(e, ctx, env)),
        Expr::Of { quant, set, tail } => eval_of(quant, set, tail, ctx, env),
        Expr::For {
            quant,
            iter,
            slot,
            body,
        } => eval_for(quant, iter, *slot, body, ctx, env),
        Expr::RuleRef(name) => ctx
            .rules
            .iter()
            .find(|(n, _)| n == name)
            .map_or(Val::Unknown, |(_, v)| v.clone()),
        Expr::Abstain(_) => Val::Unknown,
    }
}

fn read_int(signed: bool, bytes: u8, big_endian: bool, at: Val, ctx: &XCtx) -> Val {
    let Val::Int(off) = at else {
        return match at {
            Val::Undef => Val::Undef,
            _ => Val::Unknown,
        };
    };
    if off < 0 {
        return Val::Undef;
    }
    let off = off as usize;
    let end = off.saturating_add(bytes as usize);
    if end > ctx.data.len() {
        return if ctx.truncated {
            Val::Unknown
        } else {
            Val::Undef
        };
    }
    let mut buf = [0u8; 8];
    buf[..bytes as usize].copy_from_slice(&ctx.data[off..end]);
    let raw = if big_endian {
        u64::from_be_bytes(buf)
    } else {
        u64::from_le_bytes(buf)
    };
    let shift = 64 - (bytes as u32) * 8;
    let v = if signed {
        ((raw << shift) as i64) >> shift
    } else {
        raw as i64
    };
    int(v)
}

fn eval_obj(path: &ObjPath, ctx: &XCtx, env: &mut Env) -> Val {
    let base = match &path.root {
        Root::Module(_) => ctx.tree.clone(),
        Root::Var(slot) => match env.objects.get(*slot as usize).cloned().flatten() {
            Some(n) => n,
            None => return Val::Unknown,
        },
    };
    let mut call =
        |owner: &Node, name: &str, args: &[Expr], vals: &[Val]| pe2::call(owner, name, args, vals);
    let mut ev = |e: &Expr| eval(e, ctx, env);
    modreg::walk_node(&base, &path.segs, &mut ev, &mut call)
}

fn eval_of(quant: &Quant, set: &OfSet, tail: &OfTail, ctx: &XCtx, env: &mut Env) -> Val {
    let Some(q) = quant_val(quant, ctx, env) else {
        return Val::Unknown;
    };
    let (mut t, mut f, mut u) = (0i64, 0i64, 0i64);
    match set {
        OfSet::Strings(ids) => {
            for id in ids {
                let v = match tail {
                    OfTail::Plain => ctx.facts.get(*id).map_or(Val::Unknown, StrFact::found),
                    OfTail::At(e) => {
                        let at = eval(e, ctx, env);
                        ctx.facts.get(*id).map_or(Val::Unknown, |x| x.found_at(at))
                    }
                    OfTail::In(lo, hi) => {
                        let lo = eval(lo, ctx, env);
                        let hi = eval(hi, ctx, env);
                        ctx.facts
                            .get(*id)
                            .map_or(Val::Unknown, |x| x.found_in(lo, hi))
                    }
                };
                count3(v, &mut t, &mut f, &mut u);
            }
            xloop::of_result(q, t, f, u, ids.len() as i64)
        }
        OfSet::Rules(names) => {
            for name in names {
                let v = ctx
                    .rules
                    .iter()
                    .find(|(n, _)| n == name)
                    .map_or(Val::Unknown, |(_, v)| v.clone());
                count3(v, &mut t, &mut f, &mut u);
            }
            xloop::of_result(q, t, f, u, names.len() as i64)
        }
    }
}

fn eval_for(quant: &Quant, iter: &Iter, slot: u8, body: &Expr, ctx: &XCtx, env: &mut Env) -> Val {
    let Some(q) = quant_val(quant, ctx, env) else {
        return Val::Unknown;
    };
    match iter {
        Iter::Range(a, b) => {
            let (Val::Int(lo), Val::Int(hi)) = (eval(a, ctx, env), eval(b, ctx, env)) else {
                return Val::Int(0);
            };
            if lo > hi {
                return Val::Int(0);
            }
            let len = (hi - lo + 1) as usize;
            xloop::run_loop(q, len, |i| {
                env.slots[slot as usize] = int(lo + i as i64);
                eval(body, ctx, env)
            })
        }
        Iter::Ints(exprs) => {
            let vals: Vec<Val> = exprs.iter().map(|e| eval(e, ctx, env)).collect();
            xloop::run_loop(q, vals.len(), |i| {
                env.slots[slot as usize] = vals[i].clone();
                eval(body, ctx, env)
            })
        }
        Iter::Strs(exprs) => {
            let vals: Vec<Val> = exprs.iter().map(|e| eval(e, ctx, env)).collect();
            xloop::run_loop(q, vals.len(), |i| {
                env.slots[slot as usize] = vals[i].clone();
                eval(body, ctx, env)
            })
        }
        Iter::Array(path) => {
            let Some(items) = obj_items(path, ctx, env) else {
                return Val::Int(0);
            };
            xloop::run_loop(q, items.len(), |i| {
                env.objects[slot as usize] = Some(items[i].clone());
                eval(body, ctx, env)
            })
        }
        Iter::Dict(path) => {
            let Some(items) = obj_items(path, ctx, env) else {
                return Val::Int(0);
            };
            xloop::run_loop(q, items.len(), |i| {
                if let Some(Node::Struct { fields, .. }) = items.get(i)
                    && let (Some((_, k)), Some((_, v))) = (
                        fields.iter().find(|(n, _)| *n == "key"),
                        fields.iter().find(|(n, _)| *n == "value"),
                    )
                {
                    env.slots[slot as usize] = k.as_val();
                    env.slots[(slot + 1) as usize] = v.as_val();
                }
                eval(body, ctx, env)
            })
        }
        Iter::Strings(ids) => {
            let ids = ids.clone();
            xloop::run_loop(q, ids.len(), |i| {
                env.current = Some(ids[i]);
                eval(body, ctx, env)
            })
        }
    }
}

fn obj_items(path: &ObjPath, ctx: &XCtx, env: &mut Env) -> Option<Vec<Node>> {
    let mut cur = match &path.root {
        Root::Module(_) => ctx.tree,
        Root::Var(slot) => env.objects.get(*slot as usize).and_then(|o| o.as_ref())?,
    };
    for seg in &path.segs {
        match seg {
            Seg::Field(name) => match cur {
                Node::Struct {
                    fields,
                    missing_undef,
                } => match fields.iter().find(|(k, _)| *k == name.as_str()) {
                    Some((_, n)) => cur = n,
                    None if *missing_undef => return None,
                    None => return None,
                },
                _ => return None,
            },
            _ => return None,
        }
    }
    cur.items()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sigscan::expr::{BinOp, CmpKind, Iter, OfSet, OfTail, StrOp};
    use crate::sigscan::xfacts::StrFact;

    fn run(e: &Expr, tree: &Node, facts: &[StrFact]) -> Val {
        let ctx = XCtx {
            data: b"abc",
            truncated: false,
            entrypoint: None,
            tree,
            facts,
            strings: &[],
            rules: &[],
            imports: &[],
        };
        eval(e, &ctx, &mut Env::default())
    }

    #[test]
    fn arithmetic_wraps_and_div_zero_is_undef() {
        let t = Node::Unknown;
        let add = Expr::Bin {
            op: BinOp::Add,
            float: false,
            a: Box::new(Expr::Int(1)),
            b: Box::new(Expr::Int(2)),
        };
        assert_eq!(run(&add, &t, &[]), Val::Int(3));
        let div = Expr::Bin {
            op: BinOp::Div,
            float: false,
            a: Box::new(Expr::Int(1)),
            b: Box::new(Expr::Int(0)),
        };
        assert_eq!(run(&div, &t, &[]), Val::Undef);
        let shl = Expr::Bin {
            op: BinOp::Shl,
            float: false,
            a: Box::new(Expr::Int(1)),
            b: Box::new(Expr::Int(64)),
        };
        assert_eq!(run(&shl, &t, &[]), Val::Int(0));
    }

    #[test]
    fn unknown_is_not_folded_to_false() {
        let t = Node::Unknown;
        let and = Expr::And(Box::new(Expr::Abstain("u".into())), Box::new(Expr::Int(0)));
        assert_eq!(run(&and, &t, &[]), Val::Int(0));
        let or = Expr::Or(Box::new(Expr::Abstain("u".into())), Box::new(Expr::Int(1)));
        assert_eq!(run(&or, &t, &[]), Val::Int(1));
        let not = Expr::Not(Box::new(Expr::Abstain("u".into())));
        assert_eq!(run(&not, &t, &[]), Val::Unknown);
    }

    #[test]
    fn string_ops_follow_official_empty_needle() {
        let t = Node::Unknown;
        let contains = Expr::StrOp(
            StrOp::Contains,
            Box::new(Expr::Str(b"abc".to_vec())),
            Box::new(Expr::Str(Vec::new())),
        );
        assert_eq!(run(&contains, &t, &[]), Val::Int(0));
        let icontains = Expr::StrOp(
            StrOp::IContains,
            Box::new(Expr::Str(b"abc".to_vec())),
            Box::new(Expr::Str(Vec::new())),
        );
        assert_eq!(run(&icontains, &t, &[]), Val::Int(1));
    }

    #[test]
    fn loops_and_of_use_raw_values() {
        let t = Node::Unknown;
        let body = Expr::Cmp {
            op: crate::sigscan::CmpOp::Eq,
            kind: CmpKind::Int,
            a: Box::new(Expr::Var(0)),
            b: Box::new(Expr::Int(0)),
        };
        let any = Expr::For {
            quant: Quant::Any,
            iter: Iter::Range(Box::new(Expr::Int(0)), Box::new(Expr::Int(2))),
            slot: 0,
            body: Box::new(body),
        };
        assert_eq!(run(&any, &t, &[]), Val::Int(1));

        let facts = [StrFact {
            hits: vec![0],
            exact: true,
            fixed_len: Some(3),
            complete: true,
        }];
        let of = Expr::Of {
            quant: Quant::All,
            set: OfSet::Strings(vec![0]),
            tail: OfTail::Plain,
        };
        assert_eq!(run(&of, &t, &facts), Val::Int(1));
    }

    #[test]
    fn truncated_filesize_is_unknown() {
        let t = Node::Unknown;
        let ctx = XCtx {
            data: b"abc",
            truncated: true,
            entrypoint: None,
            tree: &t,
            facts: &[],
            strings: &[],
            rules: &[],
            imports: &[],
        };
        assert_eq!(
            eval(&Expr::Filesize, &ctx, &mut Env::default()),
            Val::Unknown
        );
    }
}
