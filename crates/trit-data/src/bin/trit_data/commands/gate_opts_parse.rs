//! `gate` 参数解析。
use super::super::die;
use super::Opts;
use std::path::PathBuf;
use trit_data::gate::Config;

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
            "--no-dedup" => {
                cfg.dedup = false;
                i += 1;
            }
            "--no-loop" => {
                cfg.loop_gate = false;
                i += 1;
            }
            other => {
                if !super::gate_lang_opts::apply(&mut cfg, other, argv, &mut i)
                    && !super::gate_label_opts::apply(&mut cfg, other, argv, &mut i)
                    && !super::gate_quality_opts::apply(&mut cfg, other, argv, &mut i)
                {
                    die(&format!("未知参数 {other}"));
                }
            }
        }
    }
    Opts { jsonl, out, cfg }
}
