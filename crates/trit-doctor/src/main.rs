//! `trit-doctor` 宿主入口：只做参数解析与两种输出（JSON / 文本表）。

use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use trit_doctor::{Report, Status, gates, run};

struct Args {
    root: Option<String>,
    json: bool,
    refresh: bool,
    fingerprint: bool,
    help: bool,
}

const USAGE: &str = "usage: trit-doctor [--root DIR] [--json] [--refresh] [--fingerprint]";

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        root: None,
        json: false,
        refresh: false,
        fingerprint: false,
        help: false,
    };
    let mut iter = env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--json" => args.json = true,
            "--refresh" => args.refresh = true,
            "--fingerprint" => args.fingerprint = true,
            "--help" | "-h" => args.help = true,
            "--root" => {
                args.root = Some(
                    iter.next()
                        .ok_or_else(|| "--root needs a directory".to_string())?,
                );
            }
            other if other.starts_with("--root=") => {
                let value = other.trim_start_matches("--root=");
                if value.is_empty() {
                    return Err("--root needs a directory".to_string());
                }
                args.root = Some(value.to_string());
            }
            other => return Err(format!("unrecognized argument: {other}")),
        }
    }
    Ok(args)
}

fn is_repo_root(path: &Path) -> bool {
    path.join("tools").join("selftest.ps1").is_file()
        && path.join("docs").join("ERRORS.md").is_file()
}

fn first_repo_ancestor(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|ancestor| is_repo_root(ancestor))
        .map(Path::to_path_buf)
}

fn default_root() -> PathBuf {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if let Some(root) = first_repo_ancestor(&cwd) {
        return root;
    }
    if let Ok(exe) = env::current_exe()
        && let Some(root) = first_repo_ancestor(&exe)
    {
        return root;
    }
    // 开发构建时二进制在 target/ 下，cwd 也可能不在仓里：用编译期清单目录兜底。
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    if let Some(root) = manifest.parent().and_then(Path::parent)
        && is_repo_root(root)
    {
        return root.to_path_buf();
    }
    cwd
}

fn ascii_json_string(text: &str) -> String {
    let quoted = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string());
    let mut output = String::with_capacity(quoted.len());
    for ch in quoted.chars() {
        let code = ch as u32;
        if code <= 0x7F {
            output.push(ch);
        } else if code <= 0xFFFF {
            output.push_str(&format!("\\u{code:04x}"));
        } else {
            let value = code - 0x1_0000;
            output.push_str(&format!(
                "\\u{:04x}\\u{:04x}",
                0xD800 + (value >> 10),
                0xDC00 + (value & 0x3FF)
            ));
        }
    }
    output
}

/// 与 Python `json.dumps(..., ensure_ascii=True)` 的默认分隔符对齐：
/// 对象/数组元素用 `, `，键值用 `: `，整份报告一行。
fn json_line(report: &Report) -> String {
    let mut output = String::new();
    output.push_str("{\"tool\": ");
    output.push_str(&ascii_json_string(report.tool));
    output.push_str(", \"at\": ");
    output.push_str(&ascii_json_string(&report.at));
    output.push_str(", \"root\": ");
    output.push_str(&ascii_json_string(&report.root));
    output.push_str(", \"faces\": [");
    for (index, face) in report.faces.iter().enumerate() {
        if index > 0 {
            output.push_str(", ");
        }
        output.push_str("{\"face\": ");
        output.push_str(&ascii_json_string(&face.face));
        output.push_str(", \"status\": ");
        output.push_str(&ascii_json_string(face.status.as_str()));
        output.push_str(", \"evidence\": ");
        output.push_str(&ascii_json_string(&face.evidence));
        output.push_str(", \"detail\": ");
        output.push_str(&ascii_json_string(&face.detail));
        output.push('}');
    }
    output.push_str("]}");
    output
}

fn print_text(report: &Report) {
    println!(
        "doctor: project health snapshot at={} (observation, not a gate)",
        report.at
    );
    println!("face      status       evidence");
    println!("--------  -----------  ---------------------------------------------------------");
    for face in &report.faces {
        println!(
            "{:<9} {:<11} {}",
            face.face,
            face.status.as_str(),
            face.evidence
        );
    }
    let [ok, warn, fail, unavail] = report.counts();
    println!(
        "summary: faces={} ok={} warn={} fail={} unavail={}",
        report.faces.len(),
        ok,
        warn,
        fail,
        unavail
    );
    for face in &report.faces {
        if face.status != Status::Ok {
            println!("  [{}] {}", face.status.as_str(), face.face);
            println!("    {}", face.detail);
        }
    }
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            println!("{message}");
            return ExitCode::from(2);
        }
    };
    if args.help {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let root = match &args.root {
        Some(raw) => PathBuf::from(raw),
        None => default_root(),
    };
    if !root.is_dir() {
        println!(
            "doctor: --root is not a directory: {}",
            args.root.as_deref().unwrap_or("")
        );
        return ExitCode::from(2);
    }
    if args.fingerprint {
        println!("{}", gates::tree_fingerprint(&root));
        return ExitCode::SUCCESS;
    }

    let mut report = run(&root, args.refresh);
    report.root = args.root.unwrap_or_else(|| ".".to_string());
    if args.json {
        println!("{}", json_line(&report));
    } else {
        print_text(&report);
    }
    ExitCode::SUCCESS
}
