//! gate 的风格 / 语义开关。

use trit_data::gate::Config;

use crate::commands::die;

pub fn apply(cfg: &mut Config, flag: &str, argv: &[String], i: &mut usize) -> bool {
    match flag {
        "--style" => {
            cfg.style.enabled = true;
            cfg.style.allow = Some(list(argv, *i));
            *i += 2;
            true
        }
        "--style-deny" => {
            cfg.style.enabled = true;
            cfg.style.deny = list(argv, *i);
            *i += 2;
            true
        }
        "--no-style" => {
            cfg.style.enabled = false;
            *i += 1;
            true
        }
        "--semantics" => {
            cfg.semantics.enabled = true;
            cfg.semantics.allow = Some(list(argv, *i));
            *i += 2;
            true
        }
        "--semantics-deny" => {
            cfg.semantics.enabled = true;
            cfg.semantics.deny = list(argv, *i);
            *i += 2;
            true
        }
        "--no-semantics" => {
            cfg.semantics.enabled = false;
            *i += 1;
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
        .unwrap_or_else(|| die("标签列表形如 formal,tutorial"))
}
