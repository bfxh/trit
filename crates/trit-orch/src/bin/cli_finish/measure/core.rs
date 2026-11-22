//! 宿主侧测量：`finish` 自己执行验收命令，执行器不参与任何字段的填写。
//!
//! 威胁模型（与完成闸一致）：执行器/模型不可信，宿主可信。因此回执里的
//! 退出状态、断言计数、nonce 全部由本模块从真实进程与真实输出产生；输入
//! JSON 只能选择"跑哪条命令"，不能声明"跑过了多少断言"。
//!
//! 边界：命令白名单只允许测试运行器形态（cargo test / go test / pytest /
//! node --test 等），但这挡不住"模型写一个只含 `assert!(true)` 的测试文件"。
//! 本模块保证的是"回执不是自报"，不是"测试有语义价值"——后者属于 claim 审计
//! 与测试质量门的职责，不能靠回执结构假装解决。

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use trit_orch::{Error, Exit, Receipt};

use crate::cli_inputs::Execution;

#[path = "io.rs"]
mod io;
use io::read_capped;

pub(crate) const DEFAULT_TIMEOUT_MS: u64 = 600_000;
pub(crate) const MAX_TIMEOUT_MS: u64 = 3_600_000;

/// 执行一条验收命令并生成宿主测量回执。
pub(crate) fn run_execution(exec: &Execution, epoch: u64, nonce: &str) -> Result<Receipt, Error> {
    let Some((program, args)) = exec.command.split_first() else {
        return Err(Error::ReceiptCommandEmpty {
            task: exec.task.clone(),
        });
    };
    ensure_test_runner(&exec.task, program, args)?;

    // 授权/新鲜度/用量必须由宿主显式给出；缺失就拒绝，而不是默认成安全值。
    let Some(authorization) = exec.authorization else {
        return Err(Error::ReceiptObservationMissing {
            task: exec.task.clone(),
            field: "authorization",
        });
    };
    let Some(freshness) = exec.freshness else {
        return Err(Error::ReceiptObservationMissing {
            task: exec.task.clone(),
            field: "freshness",
        });
    };
    let Some(usage) = exec.usage else {
        return Err(Error::ReceiptObservationMissing {
            task: exec.task.clone(),
            field: "usage",
        });
    };

    let test_digest = if exec.test_files.is_empty() {
        None
    } else {
        Some(trit_orch::test_files_digest(&exec.test_files)?)
    };

    let timeout = Duration::from_millis(
        exec.timeout_ms
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .min(MAX_TIMEOUT_MS),
    );
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| Error::ReceiptExecution {
            task: exec.task.clone(),
            message: error.to_string(),
        })?;

    let mut stdout = child.stdout.take().expect("stdout is piped");
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let out_reader = std::thread::spawn(move || read_capped(&mut stdout));
    let err_reader = std::thread::spawn(move || {
        let _ = read_capped(&mut stderr);
    });

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => {
                let _ = child.kill();
                return Err(Error::ReceiptExecution {
                    task: exec.task.clone(),
                    message: error.to_string(),
                });
            }
        }
    };

    let stdout = out_reader.join().unwrap_or_default();
    let _ = err_reader.join();
    let text = String::from_utf8_lossy(&stdout);
    let (passed, failed) = parse_assertions(&text);
    let exit = match status {
        Some(status) => match status.code() {
            Some(code) => Exit::Completed { code },
            None => Exit::Signalled,
        },
        None => Exit::TimedOut,
    };
    Ok(Receipt::host_measured(epoch, exit, passed, failed)
        .with_observation(authorization, freshness, usage)
        .with_test_digest(test_digest)
        .with_audit(nonce, exec.command.clone()))
}

/// 只允许测试运行器形态的命令。拒绝任意 shell/解释器/程序，避免"宿主执行了
/// 模型给的一条 echo 命令，然后把 echo 输出当证据"。
fn ensure_test_runner(task: &str, program: &str, args: &[String]) -> Result<(), Error> {
    // 只接受 bare 程序名：`./node` 可以是仓库里的假程序，白名单会被它骗过。
    if program.contains('/') || program.contains('\\') {
        return Err(Error::ReceiptCommandNotAllowed {
            task: task.to_owned(),
            program: program.to_owned(),
        });
    }
    let raw_name = Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let lower = raw_name.to_ascii_lowercase();
    let name = lower.strip_suffix(".exe").unwrap_or(&lower);
    let first = args.first().map(|arg| arg.to_ascii_lowercase());
    let first = first.as_deref();
    let second = args.get(1).map(|arg| arg.to_ascii_lowercase());
    let allowed = match name {
        "cargo" => matches!(first, Some("test") | Some("nextest")),
        "go" => first == Some("test"),
        "node" => args.iter().any(|arg| arg == "--test"),
        "pytest" | "py.test" => true,
        "python" | "python3" => first == Some("-m") && second.as_deref() == Some("pytest"),
        "dotnet" => first == Some("test"),
        "npm" | "pnpm" | "yarn" | "bun" | "deno" => first == Some("test"),
        "make" => first == Some("test"),
        _ => false,
    };
    if !allowed {
        return Err(Error::ReceiptCommandNotAllowed {
            task: task.to_owned(),
            program: program.to_owned(),
        });
    }
    Ok(())
}

/// 从真实测试输出里累加断言计数。只识别各运行器的固定摘要形态，不认任意
/// `N passed` 文本之外的自由格式；解析不到就是 0/0，完成闸会按零断言拒绝。
pub(crate) fn parse_assertions(text: &str) -> (u32, u32) {
    let mut passed = 0u32;
    let mut failed = 0u32;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("test result:") {
            // cargo test: "test result: ok. 5 passed; 0 failed; 0 ignored; ..."
            let (p, f) = scan_counts(rest);
            passed = passed.saturating_add(p);
            failed = failed.saturating_add(f);
        } else if line.starts_with("Summary") && line.contains("tests run:") {
            // cargo nextest: "Summary [ 0.12s] 5 tests run: 4 passed, 1 failed, ..."
            let (p, f) = scan_counts(line);
            passed = passed.saturating_add(p);
            failed = failed.saturating_add(f);
        } else if let Some(rest) = line
            .trim_start_matches(|c: char| !c.is_ascii_alphanumeric() && c != '#')
            .strip_prefix("# pass ")
        {
            // node --test (TAP): "# pass 5"
            passed = passed.saturating_add(rest.trim().parse().unwrap_or(0));
        } else if let Some(rest) = line
            .trim_start_matches(|c: char| !c.is_ascii_alphanumeric() && c != '#')
            .strip_prefix("# fail ")
        {
            failed = failed.saturating_add(rest.trim().parse().unwrap_or(0));
        } else if let Some(rest) = line
            .trim_start_matches(|c: char| !c.is_ascii_alphanumeric() && c != '#')
            .strip_prefix("pass ")
        {
            // node --test (spec reporter): "ℹ pass 2"
            passed = passed.saturating_add(rest.trim().parse().unwrap_or(0));
        } else if let Some(rest) = line
            .trim_start_matches(|c: char| !c.is_ascii_alphanumeric() && c != '#')
            .strip_prefix("fail ")
        {
            failed = failed.saturating_add(rest.trim().parse().unwrap_or(0));
        } else if line.starts_with("--- PASS:") {
            passed = passed.saturating_add(1);
        } else if line.starts_with("--- FAIL:") {
            failed = failed.saturating_add(1);
        } else if line.starts_with("Passed!") || line.starts_with("Failed!") {
            // dotnet test: "Passed!  - Failed: 0, Passed: 5, Skipped: 0, ..."
            failed = failed.saturating_add(after_label(line, "Failed:").unwrap_or(0));
            passed = passed.saturating_add(after_label(line, "Passed:").unwrap_or(0));
        } else if (line.starts_with('=') || line.contains(" in "))
            && (line.contains(" passed") || line.contains(" failed"))
        {
            // pytest 摘要："2 passed, 1 failed in 0.12s"
            let (p, f) = scan_counts(line);
            passed = passed.saturating_add(p);
            failed = failed.saturating_add(f);
        }
    }
    (passed, failed)
}

fn scan_counts(line: &str) -> (u32, u32) {
    let mut passed = 0u32;
    let mut failed = 0u32;
    let words: Vec<String> = line
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| word.to_ascii_lowercase())
        .collect();
    for pair in words.windows(2) {
        let Ok(count) = pair[0].parse::<u32>() else {
            continue;
        };
        match pair[1].as_str() {
            "passed" => passed = passed.saturating_add(count),
            "failed" => failed = failed.saturating_add(count),
            _ => {}
        }
    }
    (passed, failed)
}

fn after_label(line: &str, label: &str) -> Option<u32> {
    let rest = line.split_once(label)?.1;
    let digits: String = rest
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::{ensure_test_runner, parse_assertions};

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn cargo_summary_counts_every_test_binary() {
        let text = "\
running 3 tests
test a ... ok
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

running 2 tests
test b ... FAILED
test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
";
        assert_eq!(parse_assertions(text), (4, 1));
    }

    #[test]
    fn pytest_node_go_and_dotnet_summaries_are_recognised() {
        assert_eq!(parse_assertions("2 passed, 1 failed in 0.12s"), (2, 1));
        assert_eq!(parse_assertions("# pass 5\n# fail 2"), (5, 2));
        assert_eq!(parse_assertions("ℹ pass 2\nℹ fail 0"), (2, 0));
        assert_eq!(
            parse_assertions("Summary [ 0.12s] 5 tests run: 4 passed, 1 failed, 0 skipped"),
            (4, 1)
        );
        assert_eq!(
            parse_assertions("--- PASS: TestA\n--- PASS: TestB\n--- FAIL: TestC"),
            (2, 1)
        );
        assert_eq!(
            parse_assertions("Passed!  - Failed: 0, Passed: 7, Skipped: 0"),
            (7, 0)
        );
    }

    #[test]
    fn freeform_text_does_not_become_evidence() {
        assert_eq!(parse_assertions("all good, 100 passed, honest"), (0, 0));
        assert_eq!(parse_assertions("not a test result: 42 passed"), (0, 0));
        assert_eq!(parse_assertions(""), (0, 0));
    }

    #[test]
    fn only_test_runner_shapes_are_allowed() {
        assert!(ensure_test_runner("a", "cargo", &argv(&["test"])).is_ok());
        assert!(ensure_test_runner("a", "cargo", &argv(&["nextest", "run"])).is_ok());
        assert!(ensure_test_runner("a", "go", &argv(&["test", "./..."])).is_ok());
        assert!(ensure_test_runner("a", "node", &argv(&["--test"])).is_ok());
        assert!(ensure_test_runner("a", "python", &argv(&["-m", "pytest"])).is_ok());
        assert!(ensure_test_runner("a", "dotnet", &argv(&["test"])).is_ok());

        assert!(ensure_test_runner("a", "cargo", &argv(&["run"])).is_err());
        assert!(ensure_test_runner("a", "./node", &argv(&["--test"])).is_err());
        assert!(ensure_test_runner("a", "echo", &argv(&["test result: ok. 99 passed"])).is_err());
        assert!(ensure_test_runner("a", "sh", &argv(&["-c", "cargo test"])).is_err());
        assert!(ensure_test_runner("a", "python", &argv(&["-c", "print('1 passed')"])).is_err());
    }
}
