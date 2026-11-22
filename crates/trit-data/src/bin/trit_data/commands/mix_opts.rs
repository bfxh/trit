//! `mix` 参数解析。

use std::collections::BTreeMap;
use std::path::PathBuf;

use trit_data::lang_filter::LangFilter;

use crate::commands::die;

use crate::commands::lang_opts;

pub struct Opts {
    pub jsonl: Option<PathBuf>,
    pub out: Option<PathBuf>,
    pub total: usize,
    pub weights: BTreeMap<String, f64>,
    pub filter: LangFilter,
}

pub fn parse(argv: &[String]) -> Opts {
    let mut jsonl: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut total = 1000usize;
    let mut weights: BTreeMap<String, f64> = BTreeMap::new();
    let mut filter = LangFilter::default();
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
            "--total" => {
                total = argv
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| die("--total 需要整数"));
                i += 2;
            }
            "--weights" => {
                for part in argv.get(i + 1).map(|s| s.as_str()).unwrap_or("").split(',') {
                    let (k, v) = part
                        .split_once('=')
                        .unwrap_or_else(|| die("--weights 形如 code=0.7,writing=0.3"));
                    weights.insert(
                        k.to_string(),
                        v.parse().unwrap_or_else(|_| die("权重需要小数")),
                    );
                }
                i += 2;
            }
            other => {
                if !lang_opts::apply(&mut filter, other, argv, &mut i) {
                    die(&format!("未知参数 {other}"));
                }
            }
        }
    }
    Opts {
        jsonl,
        out,
        total,
        weights,
        filter,
    }
}
