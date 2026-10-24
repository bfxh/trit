//! `--manifest` 的执行面静态检查。
//!
//! `trit-build --manifest jobs.json` 对 `kind: "binary"` 的任务直接
//! `Command::new(&cmd[0]).args(&cmd[1..])`，**没有 allowlist、没有沙箱**。
//! 也就是说：清单本身就是代码执行入口。这个检查不拦截任何东西，只把「这份清单会跑什么」
//! 摆出来（shell 包装、绝对路径、网络工具、cwd 越界），供评审与 CI 判断可信度。

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

const SHELLS: [&str; 8] = [
    "sh",
    "bash",
    "zsh",
    "dash",
    "cmd",
    "cmd.exe",
    "powershell",
    "pwsh",
];
const NETWORK: [&str; 6] = ["curl", "wget", "nc", "ncat", "iwr", "invoke-webrequest"];

fn basename(path: &str) -> String {
    path.replace('\\', "/")
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .to_ascii_lowercase()
}

fn is_shell(command: &str) -> bool {
    let name = basename(command);
    let name = name.strip_suffix(".exe").unwrap_or(&name).to_string();
    SHELLS.contains(&name.as_str())
}

fn is_network(command: &str) -> bool {
    let name = basename(command);
    let name = name.strip_suffix(".exe").unwrap_or(&name).to_string();
    NETWORK.contains(&name.as_str())
}

fn is_absolute(path: &str) -> bool {
    path.starts_with('/') || path.starts_with('\\') || path.get(1..2) == Some(":")
}

#[derive(Debug, Clone)]
pub struct BinaryJob {
    pub name: String,
    pub cmd: Vec<String>,
    pub cwd: Option<String>,
    pub flags: Vec<&'static str>,
}

#[derive(Debug, Clone)]
pub struct ManifestReport {
    pub total: usize,
    pub ternary: usize,
    pub binaries: Vec<BinaryJob>,
}

pub fn inspect(manifest: &Value) -> Result<ManifestReport> {
    let jobs = manifest
        .get("jobs")
        .and_then(Value::as_array)
        .context("清单缺少 jobs 数组")?;
    let mut report = ManifestReport {
        total: jobs.len(),
        ternary: 0,
        binaries: Vec::new(),
    };
    for (index, job) in jobs.iter().enumerate() {
        let kind = job
            .get("kind")
            .and_then(Value::as_str)
            .with_context(|| format!("jobs[{index}] 缺 kind"))?;
        if kind != "binary" {
            report.ternary += 1;
            continue;
        }
        let cmd: Vec<String> = job
            .get("cmd")
            .and_then(Value::as_array)
            .with_context(|| format!("jobs[{index}] 的 binary 任务缺 cmd 数组"))?
            .iter()
            .map(|value| value.as_str().unwrap_or_default().to_string())
            .collect();
        if cmd.is_empty() {
            bail!("jobs[{index}] 的 cmd 为空");
        }
        let name = job
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("jobs[{index}]"));
        let cwd = job.get("cwd").and_then(Value::as_str).map(str::to_string);
        let mut flags = vec!["executes-commands"];
        if is_shell(&cmd[0]) {
            flags.push("shell-wrapper");
        }
        if is_network(&cmd[0]) {
            flags.push("network-tool");
        }
        if is_absolute(&cmd[0]) {
            flags.push("absolute-path");
        }
        if let Some(cwd) = &cwd
            && (is_absolute(cwd) || cwd.contains(".."))
        {
            flags.push("cwd-escapes");
        }
        report.binaries.push(BinaryJob {
            name,
            cmd,
            cwd,
            flags,
        });
    }
    Ok(report)
}

pub fn run(manifest_text: &str) -> Result<(Value, bool)> {
    let manifest: Value = serde_json::from_str(manifest_text).context("清单不是合法 JSON")?;
    let report = inspect(&manifest)?;
    let jobs: Vec<Value> = report
        .binaries
        .iter()
        .map(|job| {
            json!({
                "name": job.name,
                "cmd": job.cmd,
                "cwd": job.cwd,
                "flags": job.flags,
            })
        })
        .collect();
    let value = json!({
        "tool": "trit-redteam",
        "check": "manifest-execution",
        "jobs": report.total,
        "ternary_jobs": report.ternary,
        "binary_jobs": report.binaries.len(),
        "binaries": jobs,
        "attribution": {
            "origin": "project",
            "area": "tool-design",
            "hint": "manifest 即代码执行入口；要收口就加命令 allowlist 或沙箱"
        },
        "note": "kind=binary 的任务会被直接执行（Command::new，无 allowlist/沙箱）；清单即代码执行入口",
    });
    Ok((value, !report.binaries.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ternary_only_manifest_has_no_findings() {
        let text = r#"{"jobs":[{"name":"a","kind":"ternary","source":"a.tri"}]}"#;
        let (report, finding) = run(text).unwrap();
        assert!(!finding, "{report}");
        assert_eq!(report["binary_jobs"], 0);
        assert_eq!(report["ternary_jobs"], 1);
    }

    #[test]
    fn flags_shell_wrappers_network_and_absolute_paths() {
        let text = r#"{"jobs":[
            {"name":"net","kind":"binary","cmd":["curl","http://example.invalid/x"]},
            {"name":"sh","kind":"binary","cmd":["bash","-c","rm -rf /tmp/x"]},
            {"name":"abs","kind":"binary","cmd":["C:\\Windows\\System32\\whoami.exe"],"cwd":"../../"}
        ]}"#;
        let (report, finding) = run(text).unwrap();
        assert!(finding);
        let binaries = report["binaries"].as_array().unwrap();
        assert_eq!(binaries.len(), 3);
        assert!(
            binaries[0]["flags"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f == "network-tool")
        );
        assert!(
            binaries[1]["flags"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f == "shell-wrapper")
        );
        let absolute = binaries[2]["flags"].as_array().unwrap();
        assert!(
            absolute.iter().any(|f| f == "absolute-path"),
            "{absolute:?}"
        );
        assert!(absolute.iter().any(|f| f == "cwd-escapes"), "{absolute:?}");
    }

    #[test]
    fn rejects_manifest_without_jobs() {
        assert!(run(r#"{"nope":[]}"#).is_err());
        assert!(run("not json").is_err());
    }
}
