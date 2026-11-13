//! 字符串算子（接缝实现）：`contains` / `startswith` / `endswith` / 大小写
//! 变体 / `iequals`。
//!
//! 语义按官方 4.5.4 实测：`contains ""` 与 `iequals ""` 不命中；`startswith ""`
//! 与 `endswith ""` 命中；大小写折叠只动 ASCII A–Z。

use super::expr::StrOp;

fn fold(b: u8) -> u8 {
    b.to_ascii_lowercase()
}

fn contains(hay: &[u8], needle: &[u8], fold_case: bool) -> bool {
    if needle.is_empty() {
        // 官方实测：contains "" 不命中；icontains "" 命中。
        return fold_case;
    }
    if needle.len() > hay.len() {
        return false;
    }
    hay.windows(needle.len()).any(|w| {
        if fold_case {
            w.iter().zip(needle).all(|(a, b)| fold(*a) == fold(*b))
        } else {
            w == needle
        }
    })
}

fn starts_with(hay: &[u8], needle: &[u8], fold_case: bool) -> bool {
    if needle.len() > hay.len() {
        return false;
    }
    if fold_case {
        hay[..needle.len()]
            .iter()
            .zip(needle)
            .all(|(a, b)| fold(*a) == fold(*b))
    } else {
        hay.starts_with(needle)
    }
}

fn ends_with(hay: &[u8], needle: &[u8], fold_case: bool) -> bool {
    if needle.len() > hay.len() {
        return false;
    }
    let tail = &hay[hay.len() - needle.len()..];
    if fold_case {
        tail.iter().zip(needle).all(|(a, b)| fold(*a) == fold(*b))
    } else {
        tail == needle
    }
}

pub fn apply(op: StrOp, a: &[u8], b: &[u8]) -> bool {
    match op {
        StrOp::Contains => contains(a, b, false),
        StrOp::IContains => contains(a, b, true),
        StrOp::StartsWith => starts_with(a, b, false),
        StrOp::IStartsWith => starts_with(a, b, true),
        StrOp::EndsWith => ends_with(a, b, false),
        StrOp::IEndsWith => ends_with(a, b, true),
        StrOp::IEquals => a.len() == b.len() && a.iter().zip(b).all(|(x, y)| fold(*x) == fold(*y)),
    }
}
