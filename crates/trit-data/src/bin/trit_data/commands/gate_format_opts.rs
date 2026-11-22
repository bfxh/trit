//! gate 的格式筛选参数。

use trit_data::gate::Config;

use crate::commands::die;

pub fn apply(cfg: &mut Config, flag: &str, argv: &[String], i: &mut usize) -> bool {
    match flag {
        "--format" => {
            cfg.formats.enabled = true;
            cfg.formats.allow = Some(list(argv, *i));
            *i += 2;
            true
        }
        "--format-deny" => {
            cfg.formats.enabled = true;
            cfg.formats.deny = list(argv, *i);
            *i += 2;
            true
        }
        "--no-format" => {
            cfg.formats.enabled = false;
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
        .unwrap_or_else(|| die("格式列表形如 json,yaml"))
}
