//! 进程行为扫描 + 自动分析 + MCP 配置体检。
//!
//! 三个子命令：
//! - `run -- <cmd...>`：起目标、采样它和子进程干了什么（进程/模块/TCP/目录写入/输出），
//!   落成 JSONL 并当场给分析结论；
//! - `analyze <events.jsonl>`：对已有事件流重放分析（离线、可复算）；
//! - `mcp <config.json>`：逐个起 MCP server 做 stdio 握手，量启动耗时、工具数、schema
//!   体积，并给出"关谁/换直连"的建议。
//!
//! 证据面边界：本机非管理员 ⇒ 内核 ETW 不可用，**文件读取、注册表、DNS 未采集**；
//! 报告里显式写"未采集"，不猜。Rust 运行时的分析/攻击不在本工具范围（后续单独立项）。

mod analyze;
mod collector;
mod event;
mod mcp;

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use event::Event;

#[derive(Parser)]
#[command(name = "trit-procscan", about = "进程行为扫描 + 自动分析 + MCP 体检")]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// 有中/高危发现就以 1 退出（可直接当 CI 门）
    #[arg(long, global = true)]
    strict: bool,
}

#[derive(Subcommand)]
enum Command {
    /// 跑一个命令，采集它和它的子进程干了什么
    Run {
        /// 额外监视这些目录的写入（可重复）
        #[arg(long = "watch")]
        watch: Vec<PathBuf>,
        /// 采样间隔（ms）
        #[arg(long, default_value_t = 500)]
        interval_ms: u64,
        /// 事件落盘路径（JSONL）
        #[arg(long)]
        events: Option<PathBuf>,
        /// 要观察的命令（`--` 之后）
        #[arg(last = true, required = true)]
        target: Vec<String>,
    },
    /// 对已有事件 JSONL 做分析
    Analyze { input: PathBuf },
    /// MCP 配置体检：逐个 server 握手，量启动/工具数/schema 体积
    Mcp {
        config: PathBuf,
        /// 单次握手超时（ms）
        #[arg(long, default_value_t = 8000)]
        timeout_ms: u64,
        /// 只看这些 server（可重复）
        #[arg(long)]
        only: Vec<String>,
    },
}

fn run_command(
    target: &[String],
    watch: &[PathBuf],
    interval_ms: u64,
    events_path: Option<&PathBuf>,
) -> Result<(serde_json::Value, bool)> {
    let options = collector::Options {
        watch: watch.to_vec(),
        interval_ms: interval_ms.max(100),
        capture_output: true,
        max_processes: 512,
    };
    let events = collector::run(target, &options)?;
    if let Some(path) = events_path {
        let mut text = String::new();
        for event in &events {
            text.push_str(&event.to_json().to_string());
            text.push('\n');
        }
        fs::write(path, text).with_context(|| format!("写 {} 失败", path.display()))?;
    }
    let report = analyze::analyze(&events);
    Ok((analyze::to_json(&report), analyze::has_findings(&report)))
}

fn analyze_file(path: &PathBuf) -> Result<(serde_json::Value, bool)> {
    let text = fs::read_to_string(path).with_context(|| format!("读 {} 失败", path.display()))?;
    let mut events = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(line).context("事件行不是合法 JSON")?;
        if let Some(event) = Event::from_json(&value) {
            events.push(event);
        }
    }
    let report = analyze::analyze(&events);
    Ok((analyze::to_json(&report), analyze::has_findings(&report)))
}

fn mcp_command(
    path: &PathBuf,
    timeout_ms: u64,
    only: &[String],
) -> Result<(serde_json::Value, bool)> {
    let text = fs::read_to_string(path).with_context(|| format!("读 {} 失败", path.display()))?;
    let configs = mcp::load_config(&text)?;
    let selected: Vec<&mcp::ServerConfig> = configs
        .iter()
        .filter(|config| only.is_empty() || only.iter().any(|name| name == &config.name))
        .collect();
    if selected.is_empty() {
        bail!("--only 过滤后没有 server 可选");
    }
    let reports: Vec<mcp::ServerReport> = selected
        .iter()
        .map(|config| mcp::profile(config, Duration::from_millis(timeout_ms)))
        .collect();

    let mut owners: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for report in &reports {
        for name in &report.tool_names {
            owners
                .entry(name.clone())
                .or_default()
                .push(report.name.clone());
        }
    }
    let cross: Vec<String> = owners
        .into_iter()
        .filter(|(_, servers)| servers.len() > 1)
        .map(|(tool, servers)| format!("{tool}: {}", servers.join(",")))
        .collect();
    let finding = mcp::has_findings(&reports, &cross);
    Ok((mcp::report_to_json(&reports, &cross), finding))
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let result = match &cli.command {
        Command::Run {
            watch,
            interval_ms,
            events,
            target,
        } => run_command(target, watch, *interval_ms, events.as_ref()),
        Command::Analyze { input } => analyze_file(input),
        Command::Mcp {
            config,
            timeout_ms,
            only,
        } => mcp_command(config, *timeout_ms, only),
    };
    match result {
        Ok((report, finding)) => {
            println!("{report}");
            if cli.strict && finding {
                return std::process::ExitCode::FAILURE;
            }
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
