//! gate 的语言 / 内容开关。

use trit_data::gate::{Config, LangPolicy};

use crate::commands::die;

pub fn apply(cfg: &mut Config, flag: &str, argv: &[String], i: &mut usize) -> bool {
    match flag {
        "--no-language" => {
            cfg.language_gate = false;
            *i += 1;
            true
        }
        "--lang-quality" => {
            cfg.lang_policy = match argv.get(*i + 1).map(String::as_str) {
                Some("off") => LangPolicy::Off,
                Some("defer") => LangPolicy::Defer,
                Some("strict") => LangPolicy::Strict,
                _ => die("--lang-quality 需要 off|defer|strict"),
            };
            *i += 2;
            true
        }
        "--no-lang-quality" => {
            cfg.lang_policy = LangPolicy::Off;
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
        "--code" | "--no-code" | "--natural" | "--no-natural" | "--code-threshold" => {
            super::gate_lang_content_opts::apply(cfg, flag, argv, i)
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
