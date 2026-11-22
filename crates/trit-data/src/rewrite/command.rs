//! 外部重写器：命令行解析、进程管道与超时。
//!
//! 边界写死在这里：文本走 stdin（UTF-8），结果走 stdout（UTF-8），
//! 退出码非 0 / 超时 / 空输出一律算失败，调用方据此 fail-closed。

use std::io::{BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::Rewriter;

/// 把命令行拆成 argv：空白分隔，单/双引号内保留空白；引号未闭合直接报错。
pub fn split_command(line: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for c in line.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                started = true;
            }
            None if c.is_whitespace() => {
                if started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            None => {
                cur.push(c);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return Err("命令引号未闭合".into());
    }
    if started {
        out.push(cur);
    }
    if out.is_empty() {
        return Err("命令为空".into());
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct CommandRewriter {
    prog: String,
    args: Vec<String>,
    timeout: Duration,
}

impl CommandRewriter {
    pub fn parse(line: &str, timeout: Duration) -> Result<Self, String> {
        if timeout.as_secs() == 0 {
            return Err("--timeout 必须 > 0 秒".into());
        }
        let mut argv = split_command(line)?;
        let prog = argv.remove(0);
        Ok(Self {
            prog,
            args: argv,
            timeout,
        })
    }

    /// 报告里的 `argv` 原件（`[程序, 参数…]`）。
    pub fn argv(&self) -> Vec<String> {
        let mut v = vec![self.prog.clone()];
        v.extend(self.args.iter().cloned());
        v
    }

    pub fn timeout_secs(&self) -> u64 {
        self.timeout.as_secs()
    }
}

impl Rewriter for CommandRewriter {
    fn rewrite(&self, text: &str) -> Result<String, String> {
        run_command(&self.prog, &self.args, text, self.timeout)
    }
}

/// 跑一次外部命令：stdin 送文本，stdout 当重写结果；stderr 只用于失败信息。
pub fn run_command(
    prog: &str,
    args: &[String],
    text: &str,
    timeout: Duration,
) -> Result<String, String> {
    let mut child = Command::new(prog)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("stdin: 无管道")?;
    let payload = text.to_string();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(payload.as_bytes());
    });
    let reader = child.stdout.take().ok_or("stdout: 无管道")?;
    let out_reader = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = BufReader::new(reader).read_to_string(&mut buf);
        buf
    });
    let err_reader = child.stderr.take().ok_or("stderr: 无管道")?;
    let err_thread = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = BufReader::new(err_reader).read_to_string(&mut buf);
        buf
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {}
            Err(e) => return Err(format!("wait: {e}")),
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("timeout: 超过 {}s", timeout.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let _ = writer.join();
    let out = out_reader.join().unwrap_or_default();
    let err = err_thread.join().unwrap_or_default();
    if status.code() != Some(0) {
        let code = status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".into());
        return Err(format!("exit: {code} {}", err.trim()));
    }
    let out = out.trim_end_matches(['\n', '\r']).to_string();
    if out.trim().is_empty() {
        return Err("empty: 重写器没有输出".into());
    }
    Ok(out)
}
