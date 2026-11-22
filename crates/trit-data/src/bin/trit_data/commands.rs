//! 子命令分发与共享辅助。

#[path = "commands/card.rs"]
mod card;
#[path = "commands/compare.rs"]
mod compare;
#[path = "commands/decontaminate.rs"]
mod decontaminate;
#[path = "commands/gate.rs"]
mod gate;
#[path = "commands/json.rs"]
mod json;
#[path = "commands/mix.rs"]
mod mix;
#[path = "commands/shared.rs"]
mod shared;

pub(crate) use shared::{created_unix, die, usage, write_lines};

pub fn run() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match argv.first().map(String::as_str) {
        Some("gate") => gate::run(&argv[1..]),
        Some("compare") => compare::run(&argv[1..]),
        Some("decontaminate") => decontaminate::run(&argv[1..]),
        Some("mix") => mix::run(&argv[1..]),
        _ => usage(),
    }
}
