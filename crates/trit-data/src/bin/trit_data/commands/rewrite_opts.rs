//! `rewrite` 的参数：`--cmd` / `--timeout` 本模块吃掉，其余原样交给 gate 的解析器。

use std::path::PathBuf;

use trit_data::gate::Config;

use crate::commands::die;

pub struct Opts {
    pub jsonl: Option<PathBuf>,
    pub out: Option<PathBuf>,
    pub cfg: Config,
    pub cmd: String,
    pub timeout_secs: u64,
}

pub fn parse(argv: &[String]) -> Opts {
    let mut rest = Vec::new();
    let mut cmd: Option<String> = None;
    let mut timeout_secs = 30u64;
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--cmd" => {
                cmd = Some(
                    argv.get(i + 1)
                        .cloned()
                        .unwrap_or_else(|| die("--cmd 需要一个重写器命令")),
                );
                i += 2;
            }
            "--timeout" => {
                timeout_secs = argv
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| die("--timeout 需要秒数"));
                i += 2;
            }
            other => {
                rest.push(other.to_string());
                i += 1;
            }
        }
    }
    let gate = crate::commands::gate::gate_opts::parse(&rest);
    Opts {
        jsonl: gate.jsonl,
        out: gate.out,
        cfg: gate.cfg,
        cmd: cmd.unwrap_or_else(|| die("--cmd 必填：重写器命令（文本走 stdin，结果走 stdout）")),
        timeout_secs,
    }
}
