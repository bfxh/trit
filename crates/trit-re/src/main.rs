//! `trit-re` 命令行：给人读的判定摘要，以及给下游吃的 JSONL 台账。

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use trit_re::{Triage, analyze, ledger};

#[derive(Parser)]
#[command(
    name = "trit-re",
    version,
    about = "二进制逆向台账：PE/ELF 结构 → 三值判定 + 四值冲突"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 分析一个或多个二进制，输出判定摘要
    Triage {
        /// 待分析文件
        #[arg(required = true, value_name = "PATH")]
        paths: Vec<PathBuf>,
        /// 输出 JSONL 台账（每行一个自洽记录）
        #[arg(long)]
        jsonl: bool,
        /// 同时打印全部事实
        #[arg(long)]
        facts: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Triage {
            paths,
            jsonl,
            facts,
        } => run(&paths, jsonl, facts),
    }
}

fn run(paths: &[PathBuf], jsonl: bool, facts: bool) -> ExitCode {
    let mut failed = false;
    for path in paths {
        match std::fs::read(path) {
            Ok(bytes) => {
                let triage = analyze(&bytes);
                if jsonl {
                    for line in ledger::jsonl(&triage) {
                        println!("{line}");
                    }
                } else {
                    print_human(path, &triage, facts);
                }
            }
            Err(error) => {
                eprintln!("读取失败 {}: {error}", path.display());
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn print_human(path: &Path, triage: &Triage, facts: bool) {
    println!("== {}", path.display());
    println!("   sha256    {}", triage.sha256);
    println!(
        "   size      {}  format {}  arch {}",
        triage.size, triage.format, triage.arch
    );
    println!(
        "   sections  {}  imports {}",
        triage.sections,
        match triage.imported_functions {
            Some(count) => count.to_string(),
            None => "?".to_string(),
        }
    );
    println!("   judgments");
    for judgment in &triage.judgments {
        println!(
            "     [{}] {} ({:?}) — {}",
            judgment.four.as_char(),
            judgment.id,
            judgment.value,
            judgment.title
        );
        println!("         {}", judgment.summary());
    }
    if !triage.notes.is_empty() {
        println!("   notes");
        for note in &triage.notes {
            println!("     - {note}");
        }
    }
    if facts {
        println!("   facts");
        for witness in triage.facts.iter() {
            println!(
                "     {:<28} {:<8} {}",
                witness.id,
                ledger::ternary_label(witness.value),
                witness.detail
            );
        }
    }
}
