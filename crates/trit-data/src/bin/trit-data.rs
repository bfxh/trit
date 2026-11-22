//! `trit-data` 子命令入口。

#[path = "trit_data/compare.rs"]
mod compare_cmd;
#[path = "trit_data/gate.rs"]
mod gate_cmd;

use std::path::Path;

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match argv.first().map(String::as_str) {
        Some("gate") => gate_cmd::run(&argv[1..]),
        Some("compare") => compare_cmd::run(&argv[1..]),
        _ => usage(),
    }
}

fn write_lines(path: &Path, lines: &[String]) {
    let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(path, body).unwrap_or_else(|e| die(&format!("写 {}: {e}", path.display())));
}

fn created_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn usage() -> ! {
    die(
        "用法: trit-data gate --jsonl <轨迹.jsonl> [--out <目录>] [--allow-unverified]
      trit-data compare --a <a.jsonl> --b <b.jsonl>",
    )
}

fn die(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(2);
}
