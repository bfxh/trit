//! `trit-data` 子命令入口。

#[path = "trit_data/commands.rs"]
mod commands;

fn main() {
    commands::run();
}
