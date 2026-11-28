//! `trit-fmt` 命令行：`--check`（只判，不写）/ `--write`（原地写）/ 无参数（stdin → stdout）。
//!
//! 退出码：0 = 全部符合；1 = 有文件需要格式化、读不到或不支持的语法；2 = 参数错误。
//! 文件按扩展名推断语法（`.json` / `.jsonc` / `.md`）；其它扩展名明确报
//! `unsupported syntax`，不猜。stdin 用 `--parser <json|markdown>` 指定（缺省 json）。

use std::io::Read;
use std::process::ExitCode;

use trit_fmt::format_path;

fn help() {
    println!(
        "trit-fmt {} —— 前沿格式化器（当前范围：JSON / Markdown 块级）

用法：
  trit-fmt --check <FILE>...        只判：有文件不符合格式就退出 1
  trit-fmt --write <FILE>...        原地写回（只改空白与标记，不改语义）
  trit-fmt <FILE>...                打印到 stdout
  trit-fmt --parser <json|markdown> 读 stdin，写到 stdout
",
        env!("CARGO_PKG_VERSION")
    );
}

fn main() -> ExitCode {
    let mut check = false;
    let mut write = false;
    let mut parser: Option<String> = None;
    let mut files: Vec<String> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--check" => check = true,
            "--write" | "-w" => write = true,
            "--parser" => match args.next() {
                Some(value) => parser = Some(value),
                None => {
                    eprintln!("trit-fmt: --parser 缺少值");
                    return ExitCode::from(2);
                }
            },
            "--help" | "-h" => {
                help();
                return ExitCode::SUCCESS;
            }
            other if other.starts_with('-') => {
                eprintln!("trit-fmt: unknown flag {other}");
                return ExitCode::from(2);
            }
            other => files.push(other.to_owned()),
        }
    }
    if check && write {
        eprintln!("trit-fmt: --check 与 --write 不能同时给");
        return ExitCode::from(2);
    }

    if files.is_empty() {
        let name = match parser.as_deref() {
            None | Some("json") => "stdin.json",
            Some("markdown") | Some("md") => "stdin.md",
            Some(other) => {
                eprintln!("trit-fmt: 不认识的 parser: {other}");
                return ExitCode::from(2);
            }
        };
        let mut source = String::new();
        if let Err(error) = std::io::stdin().read_to_string(&mut source) {
            eprintln!("trit-fmt: 读 stdin 失败: {error}");
            return ExitCode::from(1);
        }
        return match format_path(name, &source) {
            Ok(out) => {
                print!("{out}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("trit-fmt: {error}");
                ExitCode::from(1)
            }
        };
    }

    let mut failed = 0usize;
    for file in &files {
        let source = match std::fs::read_to_string(file) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("trit-fmt: {file}: 读不到: {error}");
                failed += 1;
                continue;
            }
        };
        let formatted = match format_path(file, &source) {
            Ok(formatted) => formatted,
            Err(error) => {
                eprintln!("trit-fmt: {file}: {error}");
                failed += 1;
                continue;
            }
        };
        if check {
            if formatted != source {
                eprintln!("trit-fmt: {file}: 需要格式化（跑 `trit-fmt --write {file}`）");
                failed += 1;
            }
            continue;
        }
        if write {
            if formatted != source
                && let Err(error) = std::fs::write(file, &formatted)
            {
                eprintln!("trit-fmt: {file}: 写入失败: {error}");
                failed += 1;
            }
            continue;
        }
        print!("{formatted}");
    }

    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
