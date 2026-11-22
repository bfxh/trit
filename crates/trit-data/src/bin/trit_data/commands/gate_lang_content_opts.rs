//! gate 的内容开关：代码/自然语言比例与阈值（从 gate_lang_opts 拆出，腾预算）。

use trit_data::gate::Config;

use crate::commands::die;

pub fn apply(cfg: &mut Config, flag: &str, argv: &[String], i: &mut usize) -> bool {
    match flag {
        "--code" => {
            cfg.code_filter = true;
            *i += 1;
            true
        }
        "--no-code" => {
            cfg.code_filter = false;
            *i += 1;
            true
        }
        "--natural" => {
            cfg.natural_filter = true;
            *i += 1;
            true
        }
        "--no-natural" => {
            cfg.natural_filter = false;
            *i += 1;
            true
        }
        "--code-threshold" => {
            cfg.code_threshold = argv
                .get(*i + 1)
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| die("--code-threshold 需要小数"));
            *i += 2;
            true
        }
        _ => false,
    }
}
