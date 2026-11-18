//! nocase 折叠（从 `regex.rs` 拆出守预算）：把模式字节集按 ASCII 折叠。
//!
//! 官方 `$r = /HELLO/ nocase` 对 `hello` 命中——模式与数据**两侧**都折叠。
//! 旧实现只在匹配时折叠数据，模式 `H` 仍是 `H`，于是漏命中。

use super::{Ast, ByteSet};

pub(super) fn fold(ast: &mut Ast) {
    match ast {
        Ast::Byte(ByteSet::Set { set, .. }) => {
            for c in b'a'..=b'z' {
                let upper = c.to_ascii_uppercase();
                let on = set[c as usize] || set[upper as usize];
                set[c as usize] = on;
                set[upper as usize] = on;
            }
        }
        Ast::Concat(v) | Ast::Alt(v) => v.iter_mut().for_each(fold),
        Ast::Repeat { node, .. } => fold(node),
        Ast::Byte(ByteSet::AnyButNl) | Ast::Assert(_) => {}
    }
}
