//! 模块对象树（接缝实现）：`Node` 与路径导航。
//!
//! `pe2::build` 造树，`pe2::call` 执行函数；下标/字典/缺字段的 undefined
//! 语义按 `docs/YARA-EXPR/01-SEMANTICS.md` §10。

#![allow(dead_code)] // 接缝占位：pe2 实现（T20–T25）后删除

use super::expr::{Expr, ObjPath, Root, Seg};
use super::xval::{Val, int};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Int(i64),
    Str(Vec<u8>),
    Undef,
    Unknown,
    Struct {
        fields: Vec<(&'static str, Node)>,
        missing_undef: bool,
    },
    Array(Vec<Node>),
    Dict(Vec<(Vec<u8>, Node)>),
}

impl Node {
    pub fn as_val(&self) -> Val {
        match self {
            Node::Int(n) => int(*n),
            Node::Str(s) => Val::Str(s.clone()),
            Node::Undef => Val::Undef,
            Node::Unknown => Val::Unknown,
            Node::Struct { .. } | Node::Array(_) | Node::Dict(_) => Val::Undef,
        }
    }

    /// 迭代源：数组元素或字典项。
    pub fn items(&self) -> Option<Vec<Node>> {
        match self {
            Node::Array(items) => Some(items.clone()),
            Node::Dict(pairs) => Some(
                pairs
                    .iter()
                    .map(|(k, v)| Node::Struct {
                        fields: vec![("key", Node::Str(k.clone())), ("value", v.clone())],
                        missing_undef: false,
                    })
                    .collect(),
            ),
            _ => None,
        }
    }
}

/// C 字符串：到第一个 NUL 为止。
pub fn c_str(bytes: &[u8]) -> &[u8] {
    match bytes.iter().position(|b| *b == 0) {
        Some(n) => &bytes[..n],
        None => bytes,
    }
}

/// 导航 `ObjPath`；`call` 用于函数段。
#[allow(clippy::type_complexity)]
pub fn walk(
    root: &Node,
    path: &ObjPath,
    eval: &mut dyn FnMut(&Expr) -> Val,
    call: &mut dyn FnMut(&Node, &str, &[Expr], &[Val]) -> Val,
) -> Val {
    match &path.root {
        Root::Module(_) => walk_node(root, &path.segs, eval, call),
        Root::Var(_) => Val::Unknown,
    }
}

/// 从给定结点开始导航（循环变量的对象根走这里）。
#[allow(clippy::type_complexity)]
pub fn walk_node(
    base: &Node,
    segs: &[Seg],
    eval: &mut dyn FnMut(&Expr) -> Val,
    call: &mut dyn FnMut(&Node, &str, &[Expr], &[Val]) -> Val,
) -> Val {
    let mut cur = base;
    let mut last_field = String::new();
    for seg in segs {
        match seg {
            Seg::Field(name) => match cur {
                Node::Struct {
                    fields,
                    missing_undef,
                } => match fields.iter().find(|(k, _)| *k == name.as_str()) {
                    Some((_, n)) => {
                        cur = n;
                        last_field = name.clone();
                    }
                    None => {
                        return if *missing_undef {
                            Val::Undef
                        } else {
                            Val::Unknown
                        };
                    }
                },
                _ => return Val::Unknown,
            },
            Seg::Index(e) => {
                let v = eval(e);
                let Node::Array(items) = cur else {
                    return Val::Unknown;
                };
                let Val::Int(i) = v else {
                    return Val::Undef;
                };
                let i = i as i32;
                if i < 0 || i as usize >= items.len() {
                    return Val::Undef;
                }
                cur = &items[i as usize];
            }
            Seg::Key(e) => {
                let v = eval(e);
                let Val::Str(k) = v else {
                    return Val::Undef;
                };
                let Node::Dict(pairs) = cur else {
                    return Val::Unknown;
                };
                let k = c_str(&k);
                return match pairs.iter().find(|(key, _)| c_str(key) == k) {
                    Some((_, n)) => n.as_val(),
                    None => Val::Undef,
                };
            }
            Seg::Call(args) => {
                let mut vals: Vec<Val> = Vec::with_capacity(args.len());
                for a in args {
                    vals.push(eval(a));
                }
                return call(cur, &last_field, args, &vals);
            }
        }
    }
    cur.as_val()
}
