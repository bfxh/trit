//! gate 的语言 / 内容开关。

use trit_data::gate::Config;

use crate::commands::die;

pub fn apply(cfg: &mut Config, flag: &str, argv: &[String], i: &mut usize) -> bool {
    match flag {
        "--no-language" => {
            cfg.language_gate = false;
            *i += 1;
            true
        }
        "--lang-allow" => {
            cfg.lang_allow = Some(list(argv, *i));
            *i += 2;
            true
        }
        "--lang-deny" => {
            cfg.lang_deny = list(argv, *i);
            *i += 2;
            true
        }
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
