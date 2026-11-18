//! 模块对象模型：`pe` 是一棵 [`Node`] 树（结构 / 数组 / 字典 / 标量），
//! 路径求值就是在树上走。三态落点：
//! - 树里**显式**放的 `Undef` = 官方没给这个字段赋值（确定的未定义）；
//! - 结构里**查不到**的字段 = 类型表里有、本实现没建模 → 记名弃权；
//! - 数组下标越界 / 字典缺键 / 非 PE 文件上的任何字段 = 确定的未定义；
//! - 缺 PE 上下文或输入截断 → 整棵树弃权（沿用既有 `pe.*` 口径）。

use super::eval::{Env, V};
use super::{Path, Root, Seg, call, yrpe};
use std::cell::OnceCell;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Node {
    Int(i64),
    Str(Vec<u8>),
    Undef,
    Unknown,
    /// 官方判定不是 PE：`is_pe` = 0，其余一律未定义。
    NotPe,
    Struct(Vec<(&'static str, Node)>),
    Array(Vec<Node>),
    Dict(Vec<(Vec<u8>, Node)>),
}

impl Node {
    pub(super) fn field(&self, name: &str) -> Option<&Node> {
        match self {
            Node::Struct(fields) => fields.iter().find(|(key, _)| *key == name).map(|(_, v)| v),
            _ => None,
        }
    }

    pub(super) fn int(&self, name: &str) -> Option<i64> {
        match self.field(name) {
            Some(Node::Int(value)) => Some(*value),
            _ => None,
        }
    }

    pub(super) fn bytes(&self, name: &str) -> Option<&[u8]> {
        match self.field(name) {
            Some(Node::Str(value)) => Some(value),
            _ => None,
        }
    }

    pub(super) fn items(&self, name: &str) -> &[Node] {
        match self.field(name) {
            Some(Node::Array(items)) => items,
            _ => &[],
        }
    }
}

pub(super) fn to_v(node: &Node) -> V<'_> {
    match node {
        Node::Int(value) => V::Int(*value),
        Node::Str(value) => V::Str(value.clone()),
        Node::Undef => V::Undef,
        Node::Unknown => V::Unknown,
        other => V::Obj(other),
    }
}

/// 惰性构建的 `pe` 树：规则不碰 `pe.*` 就不解析；整次扫描只建一次
/// （所有规则共用）。
pub(in crate::sigscan) struct PeLazy<'a> {
    data: &'a [u8],
    usable: bool,
    cell: OnceCell<Node>,
}

impl<'a> PeLazy<'a> {
    pub(in crate::sigscan) fn new(data: &'a [u8], truncated: bool, has_context: bool) -> Self {
        Self {
            data,
            usable: has_context && !truncated,
            cell: OnceCell::new(),
        }
    }

    pub(super) fn root(&self) -> &Node {
        self.cell.get_or_init(|| {
            if self.usable {
                yrpe::build(self.data)
            } else {
                Node::Unknown
            }
        })
    }

    pub(super) fn data(&self) -> &'a [u8] {
        self.data
    }
}

fn field<'a>(holder: &V<'a>, name: &str) -> V<'a> {
    match holder {
        V::Obj(Node::NotPe) if name == "is_pe" => V::Int(0),
        V::Obj(Node::NotPe) | V::Undef => V::Undef,
        V::Obj(node @ Node::Struct(_)) => node.field(name).map_or(V::Unknown, to_v),
        _ => V::Unknown,
    }
}

/// 数组下标：官方把下标截成 C `int`（探针：`pe.sections[4294967296]`
/// 落到第 0 项、`[4294967295]` 未定义）——照此镜像。
fn index<'a>(holder: &V<'a>, at: &V) -> V<'a> {
    match (holder, at) {
        (V::Undef | V::Obj(Node::NotPe), _) | (_, V::Undef) => V::Undef,
        (V::Obj(Node::Array(items)), V::Int(at)) => usize::try_from(*at as i32)
            .ok()
            .and_then(|at| items.get(at))
            .map_or(V::Undef, to_v),
        _ => V::Unknown,
    }
}

fn lookup<'a>(holder: &V<'a>, key: &V) -> V<'a> {
    match (holder, key) {
        (V::Undef | V::Obj(Node::NotPe), _) | (_, V::Undef) => V::Undef,
        // 官方按 C 串查键：含 NUL 的键会被截断——不模拟，弃权
        (V::Obj(Node::Dict(_)), V::Str(key)) if key.contains(&0) => V::Unknown,
        (V::Obj(Node::Dict(pairs)), V::Str(key)) => pairs
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map_or(V::Undef, |(_, value)| to_v(value)),
        _ => V::Unknown,
    }
}

impl<'a> Env<'a> {
    pub(super) fn path(&mut self, path: &Path) -> V<'a> {
        let mut holder = match path.root {
            Root::Pe => to_v(self.pe.root()),
            Root::Var(slot) => self.slots.get(slot).cloned().unwrap_or(V::Unknown),
        };
        let mut segs = path.segs.iter().peekable();
        while let Some(seg) = segs.next() {
            holder = match seg {
                Seg::Field(name) => match segs.peek() {
                    Some(Seg::Call(args)) => {
                        segs.next();
                        call::invoke(self, &holder, name, args)
                    }
                    _ => field(&holder, name),
                },
                Seg::Index(at) => {
                    let at = self.eval(at);
                    index(&holder, &at)
                }
                Seg::Key(key) => {
                    let key = self.eval(key);
                    lookup(&holder, &key)
                }
                Seg::Call(_) => V::Unknown,
            };
        }
        holder
    }
}
