//! `trit-fmt` 命令行：`--check`（只判，不写）/ `--write`（原地写）/ 无参数（stdin → stdout）。
//!
//! 退出码：0 = 全部符合；1 = 有文件需要格式化、读不到或不支持的语法；2 = 参数错误。
//! 文件按扩展名推断语法（`.json` / `.jsonc` / `.md`）；其它扩展名明确报
//! `unsupported syntax`，不猜。stdin 用 `--parser <json|markdown>` 指定（缺省 json）。

use std::io::Read;
use std::process::ExitCode;

use trit_fmt::format_path;

const HELP: &str = include_str!("help.txt");

fn fail(message: &str, code: u8) -> ExitCode {
    eprintln!("trit-fmt: {message}");
    ExitCode::from(code)
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
            "--parser" => parser = args.next(),
            "--help" | "-h" => {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
            other if other.starts_with('-') => return fail(&format!("unknown flag {other}"), 2),
            other => files.push(other.to_owned()),
        }
    }
    if check && write {
        return fail("--check 与 --write 不能同时给", 2);
    }
    let parser = parser.as_deref().unwrap_or("json");
    if !matches!(parser, "json" | "markdown" | "md") {
        return fail(&format!("不认识的 parser: {parser}"), 2);
    }
    if files.is_empty() {
        let mut source = String::new();
        if let Err(error) = std::io::stdin().read_to_string(&mut source) {
            return fail(&format!("读 stdin 失败: {error}"), 1);
        }
        let name = match parser {
            "json" => "stdin.json",
            _ => "stdin.md",
        };
        return match format_path(name, &source) {
            Ok(out) => {
                print!("{out}");
                ExitCode::SUCCESS
            }
            Err(error) => fail(&format!("{error}"), 1),
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

    ExitCode::from(if failed == 0 { 0 } else { 1 })
}
