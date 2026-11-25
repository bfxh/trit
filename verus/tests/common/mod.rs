#![allow(dead_code)]

//! 集成测试共用的 K3 编码/解码辅助；`bridge.rs` 与 `refinement.rs` 各自 `mod common;`。

use trit_core::value::Ternary;
use trit_verified::k3::Trit;
use trit_verified::table;

pub fn code(t: Ternary) -> u8 {
    match t {
        Ternary::False => table::F,
        Ternary::Unknown => table::U,
        Ternary::True => table::T,
    }
}

pub fn decode(c: u8) -> Ternary {
    match c {
        table::F => Ternary::False,
        table::U => Ternary::Unknown,
        table::T => Ternary::True,
        _ => panic!("非法 K3 编码 {c}"),
    }
}

pub fn trit(c: u8) -> Trit {
    match c {
        table::F => Trit::F,
        table::U => Trit::U,
        table::T => Trit::T,
        _ => panic!("非法 K3 编码 {c}"),
    }
}

pub fn trit_code(t: Trit) -> u8 {
    match t {
        Trit::F => table::F,
        Trit::U => table::U,
        Trit::T => table::T,
    }
}
