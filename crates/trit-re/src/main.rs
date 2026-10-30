//! `trit-re` 命令行：给人读的判定摘要，以及给下游吃的 JSONL 台账。

use std::io::{self, Write};
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
        /// 打印每个导入的调用点明细
        #[arg(long)]
        xref: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Triage {
            paths,
            jsonl,
            facts,
            xref,
        } => run(&paths, jsonl, facts, xref),
    }
}

fn run(paths: &[PathBuf], jsonl: bool, facts: bool, xref: bool) -> ExitCode {
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    let mut failed = false;
    for path in paths {
        let result = match std::fs::read(path) {
            Ok(bytes) => {
                let triage = analyze(&bytes);
                if jsonl {
                    emit_jsonl(&mut out, &triage)
                } else {
                    emit_human(&mut out, path, &triage, facts, xref)
                }
            }
            Err(error) => {
                eprintln!("读取失败 {}: {error}", path.display());
                failed = true;
                continue;
            }
        };
        if let Err(error) = result {
            // `trit-re ... | head` 会提前关管道；这不是错误，安静退出。
            if error.kind() == io::ErrorKind::BrokenPipe {
                return ExitCode::SUCCESS;
            }
            eprintln!("写 stdout 失败: {error}");
            return ExitCode::from(1);
        }
    }
    if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn emit_jsonl(out: &mut impl Write, triage: &Triage) -> io::Result<()> {
    for line in ledger::jsonl(triage) {
        writeln!(out, "{line}")?;
    }
    Ok(())
}

fn emit_human(
    out: &mut impl Write,
    path: &Path,
    triage: &Triage,
    facts: bool,
    xref: bool,
) -> io::Result<()> {
    writeln!(out, "== {}", path.display())?;
    writeln!(out, "   sha256    {}", triage.sha256)?;
    writeln!(
        out,
        "   size      {}  format {}  arch {}",
        triage.size, triage.format, triage.arch
    )?;
    writeln!(
        out,
        "   sections  {}  imports {}",
        triage.sections,
        match triage.imported_functions {
            Some(count) => count.to_string(),
            None => "?".to_string(),
        }
    )?;
    if let Some(report) = &triage.xref {
        writeln!(
            out,
            "   xref      complete={} scanned={}  有调用点 {}  无调用点 {}  未知 {}",
            report.complete,
            report.scanned_bytes,
            report.referenced(),
            report.unreferenced(),
            report.unknown()
        )?;
    }
    writeln!(out, "   judgments")?;
    for judgment in &triage.judgments {
        writeln!(
            out,
            "     [{}] {} ({:?}) — {}",
            judgment.four.as_char(),
            judgment.id,
            judgment.value,
            judgment.title
        )?;
        writeln!(out, "         {}", judgment.summary())?;
    }
    if xref && let Some(report) = &triage.xref {
        writeln!(out, "   xref detail")?;
        for row in &report.imports {
            let sites: Vec<String> = row
                .sites
                .iter()
                .take(4)
                .map(|site| format!("0x{:x}({})", site.addr, site.kind.as_str()))
                .collect();
            writeln!(
                out,
                "     {} {:<40} {:>3} 处  {}",
                row.status_char(),
                row.import,
                row.sites.len(),
                sites.join(", ")
            )?;
        }
    }
    if !triage.notes.is_empty() {
        writeln!(out, "   notes")?;
        for note in &triage.notes {
            writeln!(out, "     - {note}")?;
        }
    }
    if facts {
        writeln!(out, "   facts")?;
        for witness in triage.facts.iter() {
            writeln!(
                out,
                "     {:<28} {:<8} {}",
                witness.id,
                ledger::ternary_label(witness.value),
                witness.detail
            )?;
        }
    }
    out.flush()
}
