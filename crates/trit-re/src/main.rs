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
        /// 打印函数级调用与能力桶
        #[arg(long)]
        functions: bool,
    },
    /// 吃 procscan 的 JSONL 事件流，采集其中的模块路径并逐个分析（N3）
    Collect {
        /// procscan 事件流 JSONL（缺省读 stdin；`-` 亦表示 stdin）
        #[arg(long, value_name = "FILE")]
        events: Option<PathBuf>,
        /// 只输出汇总行（默认输出每模块台账行 + 汇总行）
        #[arg(long)]
        summary_only: bool,
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
            functions,
        } => run(&paths, jsonl, facts, xref, functions),
        Command::Collect {
            events,
            summary_only,
        } => run_collect(events.as_deref(), summary_only),
    }
}

fn run(paths: &[PathBuf], jsonl: bool, facts: bool, xref: bool, functions: bool) -> ExitCode {
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
                    emit_human(&mut out, path, &triage, facts, xref, functions)
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
    functions: bool,
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
    if let Some(report) = &triage.functions {
        writeln!(
            out,
            "   functions {} 个  边界={}  归属调用点 {}  未归属 {}",
            report.total_functions,
            if report.exact { "exact" } else { "heuristic" },
            report.attributed_sites,
            report.unattributed_sites
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
    if functions && let Some(report) = &triage.functions {
        writeln!(out, "   function detail")?;
        for calls in &report.functions {
            writeln!(
                out,
                "     [{}] 0x{:x}+0x{:x} {} imports={} buckets={}",
                calls.function.source.as_str(),
                calls.function.start,
                calls.function.end.saturating_sub(calls.function.start),
                calls.function.name.as_deref().unwrap_or("-"),
                calls.imports.len(),
                calls.buckets.join(",")
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

/// `trit-re collect`：procscan 事件流 → 模块台账 + 采集汇总。
///
/// 事件流来自文件或 stdin（`trit-procscan … | trit-re collect` 是预期管线）；
/// 输出走 stdout（逐行 JSONL），stderr 给一行人读摘要。
fn run_collect(events: Option<&Path>, summary_only: bool) -> ExitCode {
    let text = match events {
        Some(path) if path.as_os_str() != "-" => match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                eprintln!("读取事件流 {} 失败: {error}", path.display());
                return ExitCode::from(1);
            }
        },
        _ => {
            let mut buffer = String::new();
            if let Err(error) = io::Read::read_to_string(&mut io::stdin(), &mut buffer) {
                eprintln!("读 stdin 失败: {error}");
                return ExitCode::from(1);
            }
            buffer
        }
    };
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    let reader = |path: &Path| std::fs::read(path);
    let (rows, summary) = trit_re::collect(&text, reader);
    let mut failed = false;
    for line in &rows {
        if summary_only && !line.contains("\"collect-summary\"") {
            continue;
        }
        if let Err(error) = writeln!(out, "{line}") {
            if error.kind() == io::ErrorKind::BrokenPipe {
                return ExitCode::SUCCESS;
            }
            eprintln!("写 stdout 失败: {error}");
            return ExitCode::from(1);
        }
    }
    if let Err(error) = out.flush()
        && error.kind() != io::ErrorKind::BrokenPipe
    {
        eprintln!("写 stdout 失败: {error}");
        failed = true;
    }
    eprintln!(
        "采集：模块事件 {} / 唯一 {}（去重 {}）→ 分析 {}，不可读 {}，非 module 事件 {}，坏行 {}，{} 字节，{} ms",
        summary.module_events,
        summary.modules,
        summary.deduped,
        summary.analyzed,
        summary.unreadable,
        summary.non_module_events,
        summary.bad_lines,
        summary.bytes,
        summary.elapsed_ms
    );
    if summary.unreadable > 0 {
        failed = true;
    }
    if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
