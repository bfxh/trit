//! `route` 路由表本体（从 server.rs 拆出，控制单文件行数预算）。

use crate::design::{self, DesignArea, PrIntake};
use crate::diag::{self, Severity};
use crate::journal::Journal;
use crate::lsp;
use crate::scan;
use crate::sync::{self, LogFrame};
use crate::taskmd;
use std::io;
use std::sync::RwLock;

pub(super) fn route(
    journal: &RwLock<Journal>,
    method: &str,
    path: &str,
    query: &[(String, String)],
    body: &str,
) -> (u16, String) {
    match (method, path) {
        ("GET", "/v1/health") => (200, "ok\n".into()),
        ("GET", "/v1/render") => (200, journal.read().expect("锁中毒").dag().render()),
        ("GET", "/v1/suspects") => {
            let j = journal.read().expect("锁中毒");
            let list: Vec<String> = j
                .dag()
                .suspects()
                .iter()
                .map(|id| format!("#{:02} {}", id.0, j.dag().node(*id).title))
                .collect();
            (200, format!("{}\n", list.join("\n")))
        }
        ("GET", "/v1/taskbar") => {
            let Some((_, v)) = query.iter().find(|(k, _)| k == "parent") else {
                return (400, "缺 parent 参数\n".into());
            };
            let Ok(parent) = v.parse::<u32>() else {
                return (400, format!("parent 非数字：{v}\n"));
            };
            let j = journal.read().expect("锁中毒");
            if parent >= j.dag().nodes().len() as u32 {
                return (404, format!("节点 #{parent:02} 不存在\n"));
            }
            let list: Vec<String> = taskmd::taskbar(&j, crate::NodeId(parent))
                .into_iter()
                .map(|(id, title)| format!("#{:02} {}", id.0, title))
                .collect();
            (200, format!("{}\n", list.join("\n")))
        }
        ("POST", "/v1/taskmd") => {
            // MD → 任务栏：body = Markdown 文本
            let mut parent: Option<u32> = None;
            let mut quarantine = false;
            let mut source = "HTTP".to_string();
            for (k, v) in query {
                match k.as_str() {
                    "parent" => parent = v.parse::<u32>().ok(),
                    "quarantine" => quarantine = v == "1" || v == "true",
                    "source" => source = v.clone(),
                    _ => {}
                }
            }
            let Some(parent) = parent else {
                return (400, "缺 parent 参数\n".into());
            };
            let r = {
                let mut j = journal.write().expect("锁中毒");
                taskmd::ingest(&mut j, crate::NodeId(parent), &source, body, quarantine)
            };
            match r {
                Ok(o) => (
                    200,
                    format!(
                        "created={} skipped={} quarantined={} zero_state_only=true\n",
                        o.created, o.skipped, o.quarantined
                    ),
                ),
                Err(e) => (400, format!("入账失败：{e}\n")),
            }
        }
        ("GET", "/v1/gate") => {
            let Some((_, v)) = query.iter().find(|(k, _)| k == "tip") else {
                return (400, "缺 tip 参数\n".into());
            };
            let Ok(tip) = v.parse::<u32>() else {
                return (400, format!("tip 非数字：{v}\n"));
            };
            let j = journal.read().expect("锁中毒");
            let len = j.dag().nodes().len() as u32;
            if tip >= len {
                return (404, format!("节点 #{tip:02} 不存在\n"));
            }
            let verdict = j.dag().node(crate::NodeId(tip)).verdict;
            let gate = j.dag().merge_gate(crate::NodeId(tip));
            (
                200,
                format!("node=#{tip:02} verdict={verdict} gate={gate}\n"),
            )
        }
        ("POST", "/v1/deltas") => match parse_frame_str(body) {
            Ok(frame) => {
                let out = {
                    let mut j = journal.write().expect("锁中毒");
                    sync::merge_into(&mut j, frame.writer, frame.stamped)
                };
                match out {
                    Ok(o) => {
                        let suspects = journal.read().expect("锁中毒").dag().suspects().len();
                        (
                            200,
                            format!(
                                "canonical={} deferred={} suspects={}\n",
                                o.canonical.len(),
                                o.deferred.len(),
                                suspects
                            ),
                        )
                    }
                    Err(e) => (500, format!("合并采纳失败：{e}\n")),
                }
            }
            Err(e) => (400, format!("帧解析失败：{e}\n")),
        },
        ("POST", "/v1/diag") => {
            // 编译器管线桥：body = rustc --message-format=json 流
            let Some((_, v)) = query.iter().find(|(k, _)| k == "node") else {
                return (400, "缺 node 参数\n".into());
            };
            let Ok(node) = v.parse::<u32>() else {
                return (400, format!("node 非数字：{v}\n"));
            };
            // 守卫：垃圾输入拒绝（避免空 POST 把节点"洗"成 +1）
            if body.trim().is_empty() || !diag::has_json(body) {
                return (
                    400,
                    "无诊断数据（应为 rustc --message-format=json 流）\n".into(),
                );
            }
            let diags = diag::parse_rustc_stream(body);
            if diags.is_empty() && !body.contains("\"success\":true") {
                return (
                    400,
                    "无诊断数据（0 条，拒绝空 POST 把节点洗成 +1）\n".into(),
                );
            }
            let verdict = diag::layer_verdict(&diags);
            let errors = diags
                .iter()
                .filter(|d| d.severity == Severity::Error)
                .count();
            let warnings = diags
                .iter()
                .filter(|d| d.severity == Severity::Warning)
                .count();
            let len = journal.read().expect("锁中毒").dag().nodes().len() as u32;
            if node >= len {
                return (404, format!("节点 #{node:02} 不存在\n"));
            }
            let applied = {
                let mut j = journal.write().expect("锁中毒");
                j.append_delta(&crate::Delta::Record {
                    id: crate::NodeId(node),
                    verdict,
                    evidence: None,
                    note: format!("编译层：errors={errors} warnings={warnings}（检查类，可为 +1）"),
                })
            };
            match applied {
                Ok(()) => (
                    200,
                    format!(
                        "diagnostics={} errors={} warnings={} layer={verdict} check_may_plus1=true\n",
                        diags.len(),
                        errors,
                        warnings
                    ),
                ),
                Err(e) => (500, format!("写入失败：{e}\n")),
            }
        }
        ("POST", "/v1/scan") => match parse_scan_body(body) {
            Ok(snippets) => {
                let refs: Vec<(crate::NodeId, &str)> =
                    snippets.iter().map(|(n, c)| (*n, c.as_str())).collect();
                let report = scan::run_scan(&refs, scan::RULES, &[]);
                let deltas = report.to_deltas("HTTP");
                let applied = (|| -> io::Result<usize> {
                    let mut j = journal.write().expect("锁中毒");
                    let mut n = 0usize;
                    for d in &deltas {
                        j.append_delta(d)
                            .map_err(|e| io::Error::other(e.to_string()))?;
                        n += 1;
                    }
                    Ok(n)
                })();
                match applied {
                    Ok(n) => (
                        200,
                        format!(
                            "scanned={} findings={} applied={} scanner_never_plus1=true\n",
                            report.scanned,
                            report.findings.len(),
                            n
                        ),
                    ),
                    Err(e) => (500, format!("写入失败：{e}\n")),
                }
            }
            Err(e) => (400, format!("{e}\n")),
        },
        ("POST", "/v1/lsp") => {
            // LSP 桥：body = publishDiagnostics 通知流（rust-analyzer 等）
            let Some((_, v)) = query.iter().find(|(k, _)| k == "node") else {
                return (400, "缺 node 参数\n".into());
            };
            let Ok(node) = v.parse::<u32>() else {
                return (400, format!("node 非数字：{v}\n"));
            };
            if body.trim().is_empty() || !diag::has_json(body) {
                return (400, "无诊断数据（应为 LSP 通知流）\n".into());
            }
            let diags = lsp::parse_lsp_stream(body)
                .into_iter()
                .map(|d| d.into_diag())
                .collect::<Vec<_>>();
            if diags.is_empty() && !body.contains("\"success\":true") {
                return (
                    400,
                    "无诊断数据（0 条，拒绝空 POST 把节点洗成 +1）\n".into(),
                );
            }
            let verdict = diag::layer_verdict(&diags);
            let errors = diags
                .iter()
                .filter(|d| d.severity == Severity::Error)
                .count();
            let warnings = diags
                .iter()
                .filter(|d| d.severity == Severity::Warning)
                .count();
            let len = journal.read().expect("锁中毒").dag().nodes().len() as u32;
            if node >= len {
                return (404, format!("节点 #{node:02} 不存在\n"));
            }
            let applied = {
                let mut j = journal.write().expect("锁中毒");
                j.append_delta(&crate::Delta::Record {
                    id: crate::NodeId(node),
                    verdict,
                    evidence: None,
                    note: format!("LSP 层：errors={errors} warnings={warnings}（检查类，可为 +1）"),
                })
            };
            match applied {
                Ok(()) => (
                    200,
                    format!(
                        "diagnostics={} errors={} warnings={} layer={verdict} check_may_plus1=true\n",
                        diags.len(),
                        errors,
                        warnings
                    ),
                ),
                Err(e) => (500, format!("写入失败：{e}\n")),
            }
        }
        ("POST", "/v1/intake") => {
            // 设计决策生产线入口：分类（新决策/执行既有/退回设计队列）
            let mut title = "untitled".to_string();
            let mut touches: Vec<DesignArea> = Vec::new();
            let mut is_new = false;
            let mut adr: Option<String> = None;
            for (k, v) in query {
                match k.as_str() {
                    "title" => title = v.clone(),
                    "areas" => {
                        touches = v.split(',').filter_map(DesignArea::parse).collect();
                    }
                    "new" => is_new = v == "1" || v == "true",
                    "adr" => adr = Some(v.clone()),
                    _ => {}
                }
            }
            let intake = PrIntake {
                title,
                touches,
                is_new_decision: is_new,
                linked_adr: adr,
                impact_summary: None,
                rollback: None,
            };
            let routing = design::classify(&intake);
            (200, format!("routing={routing:?}\n"))
        }
        ("GET", _) | ("POST", _) => (404, "未知路径\n".into()),
        _ => (405, "方法不允许\n".into()),
    }
}

pub(super) fn parse_frame_str(body: &str) -> Result<LogFrame, String> {
    sync::parse_frame(body)
}

/// 扫描入账请求行格式：`SNIP\t<node>\t<转义代码>`（esc 转义，与日志同风格）。
pub(super) fn parse_scan_body(body: &str) -> Result<Vec<(crate::NodeId, String)>, String> {
    let mut snippets = Vec::new();
    for line in body.lines().filter(|l| !l.trim().is_empty()) {
        let mut f = line.split('\t');
        match (f.next(), f.next(), f.next(), f.next()) {
            (Some("SNIP"), Some(node), Some(code), None) => {
                let n = node
                    .parse::<u32>()
                    .map_err(|_| format!("非法节点号：{node:?}"))?;
                let c = crate::delta::unesc(code)?;
                snippets.push((crate::NodeId(n), c));
            }
            _ => return Err("行格式应为 SNIP\\t<node>\\t<code>".into()),
        }
    }
    Ok(snippets)
}
