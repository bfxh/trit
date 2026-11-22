//! `gate` 参数解析。

use std::path::PathBuf;

use trit_data::gate::Config;

use super::super::die;
use super::Opts;

pub fn parse(argv: &[String]) -> Opts {
    let mut jsonl: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut cfg = Config::default();
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--jsonl" => {
                jsonl = argv.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--out" => {
                out = argv.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--allow-unverified" => {
                cfg.require_verified = false;
                i += 1;
            }
            "--min-thought" => {
                cfg.min_thought_rate = argv
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| die("--min-thought 需要小数"));
                i += 2;
            }
            "--no-language" => {
                cfg.language_gate = false;
                i += 1;
            }
            "--lang-allow" => {
                cfg.lang_allow = Some(list(argv, i));
                i += 2;
            }
            "--lang-deny" => {
                cfg.lang_deny = list(argv, i);
                i += 2;
            }
            "--no-dedup" => {
                cfg.dedup = false;
                i += 1;
            }
            "--no-loop" => {
                cfg.loop_gate = false;
                i += 1;
            }
            other => die(&format!("未知参数 {other}")),
        }
    }
    Opts { jsonl, out, cfg }
}

fn list(argv: &[String], i: usize) -> Vec<String> {
    argv.get(i + 1)
        .map(|s| {
            s.split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect()
        })
        .unwrap_or_else(|| die("语言列表形如 zh,en"))
}
