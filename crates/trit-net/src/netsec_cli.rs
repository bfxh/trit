//! `netsec` 的 CLI 子命令（probe / bridge）——从 `main.rs` 拆出，让每个文件
//! 都待在模块预算内。arg 解析是薄层；判决逻辑都在库里。

use trit_net::netsec::attack::AttackGraph;
use trit_net::netsec::probe::{Allowlist, ProbeConfig, attach_probe, findings_from_jsonl, run};

pub fn probe_cmd(args: &[String]) -> Result<(), String> {
    let mut path: Option<String> = None;
    let mut connect_timeout_ms = 1_500u64;
    let mut delay_ms = 100u64;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].clone();
        match flag.as_str() {
            "--connect-timeout" | "--delay" => {
                index += 1;
                let raw = args.get(index).ok_or_else(|| format!("{flag} 缺少参数"))?;
                let parsed: u64 = raw
                    .parse()
                    .map_err(|_| format!("{flag} 不是整数（毫秒）"))?;
                if flag == "--connect-timeout" {
                    connect_timeout_ms = parsed;
                } else {
                    delay_ms = parsed;
                }
            }
            other if other.starts_with("--") => return Err(format!("未知参数 {other}")),
            other => {
                if path.is_some() {
                    return Err(format!("多余的参数 {other}"));
                }
                path = Some(other.to_string());
            }
        }
        index += 1;
    }
    let path = path.ok_or(
        "probe 需要 allowlist JSON 路径（{\"targets\":[{\"ip\":\"127.0.0.1\",\"ports\":[..]}]}；\
         只接受字面 IP、只对环回/私网发包）",
    )?;
    let text =
        std::fs::read_to_string(&path).map_err(|error| format!("读取 {path} 失败：{error}"))?;
    let allowlist = Allowlist::from_json(&text).map_err(|error| error.to_string())?;
    let config = ProbeConfig {
        connect_timeout: std::time::Duration::from_millis(connect_timeout_ms),
        delay: std::time::Duration::from_millis(delay_ms),
        ..ProbeConfig::default()
    };
    let report = run(&allowlist, &config).map_err(|error| error.to_string())?;
    for finding in &report.findings {
        println!(
            "{}",
            serde_json::to_string(finding).map_err(|error| error.to_string())?
        );
    }
    eprintln!(
        "计划 {} / 执行 {} / 截断 {}",
        report.planned, report.probed, report.truncated
    );
    Ok(())
}

pub fn bridge_cmd(args: &[String]) -> Result<(), String> {
    let mut graph_path: Option<String> = None;
    let mut report_path: Option<String> = None;
    let mut station = "probe-station".to_string();
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].clone();
        match flag.as_str() {
            "--station" => {
                index += 1;
                station = args.get(index).ok_or("--station 缺少参数")?.clone();
            }
            other if other.starts_with("--") => return Err(format!("未知参数 {other}")),
            other => {
                if graph_path.is_some() {
                    if report_path.is_some() {
                        return Err(format!("多余的参数 {other}"));
                    }
                    report_path = Some(other.to_string());
                } else {
                    graph_path = Some(other.to_string());
                }
            }
        }
        index += 1;
    }
    let graph_path = graph_path.ok_or("bridge 需要 <攻击图.jsonl> <探测报告.jsonl>")?;
    let report_path = report_path.ok_or("bridge 需要 <探测报告.jsonl>（probe 的 JSONL 输出）")?;
    let graph_text = std::fs::read_to_string(&graph_path)
        .map_err(|error| format!("读取 {graph_path} 失败：{error}"))?;
    let report_text = std::fs::read_to_string(&report_path)
        .map_err(|error| format!("读取 {report_path} 失败：{error}"))?;
    let mut graph = AttackGraph::from_jsonl(&graph_text).map_err(|error| error.to_string())?;
    let findings = findings_from_jsonl(&report_text).map_err(|error| error.to_string())?;
    let summary =
        attach_probe(&mut graph, &findings, &station).map_err(|error| error.to_string())?;
    print!("{}", graph.to_jsonl());
    eprintln!(
        "登记完成：主机 {}、服务 {}、边 {}（同 label/同端点 LWW 覆盖，重跑幂等）",
        summary.hosts, summary.services, summary.edges
    );
    Ok(())
}

/// `trit-net drift <基线.json> <当前.json> [--max-fpr-delta F] [--max-tpr-delta F]
/// [--max-abstain-delta F]`：漂移门——基线与当前校准读数按 band 对账，
/// 变坏超阈值即红（退出码非零），结构不对齐一律红（fail-closed）。
pub fn drift_cmd(args: &[String]) -> Result<(), String> {
    use trit_net::netsec::drift::{DriftLimits, drift_check, parse_report};

    let mut baseline: Option<String> = None;
    let mut current: Option<String> = None;
    let mut limits = DriftLimits::default();
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].clone();
        match flag.as_str() {
            "--max-fpr-delta" | "--max-tpr-delta" | "--max-abstain-delta" => {
                index += 1;
                let raw = args.get(index).ok_or_else(|| format!("{flag} 缺少参数"))?;
                let value: f64 = raw.parse().map_err(|_| format!("{flag} 不是数字"))?;
                match flag.as_str() {
                    "--max-fpr-delta" => limits.max_fpr_delta = value,
                    "--max-tpr-delta" => limits.max_tpr_delta = value,
                    _ => limits.max_abstain_delta = value,
                }
            }
            other if other.starts_with("--") => return Err(format!("未知参数 {other}")),
            other => {
                if baseline.is_some() {
                    if current.is_some() {
                        return Err(format!("多余的参数 {other}"));
                    }
                    current = Some(other.to_string());
                } else {
                    baseline = Some(other.to_string());
                }
            }
        }
        index += 1;
    }
    let baseline =
        baseline.ok_or("drift 需要 <基线.json> <当前.json>（calibrate --json 的输出）")?;
    let current = current.ok_or("drift 需要 <当前.json>（calibrate --json 的输出）")?;
    let read = |path: &String| {
        std::fs::read_to_string(path).map_err(|error| format!("读取 {path} 失败：{error}"))
    };
    let base = parse_report(&read(&baseline)?).map_err(|error| format!("{baseline}：{error}"))?;
    let now = parse_report(&read(&current)?).map_err(|error| format!("{current}：{error}"))?;
    let verdict = drift_check(&base, &now, limits);
    println!(
        "{}",
        serde_json::to_string(&verdict).map_err(|error| error.to_string())?
    );
    if verdict.verdict == "red" {
        return Err(format!(
            "漂移门红：{} 条结构性问题，{} 个 band 触红（详见上方 JSON）",
            verdict.structural.len(),
            verdict
                .bands
                .iter()
                .filter(|band| !band.reasons.is_empty())
                .count()
        ));
    }
    Ok(())
}
