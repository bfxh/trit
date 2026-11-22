//! gate 的位宽 / 多值筛选参数。

use trit_data::gate::Config;

use crate::commands::die;

pub fn apply(cfg: &mut Config, flag: &str, argv: &[String], i: &mut usize) -> bool {
    match flag {
        "--bits" => {
            cfg.bits.enabled = true;
            cfg.bits.allow = Some(list(argv, *i));
            *i += 2;
            true
        }
        "--bits-deny" => {
            cfg.bits.enabled = true;
            cfg.bits.deny = list(argv, *i);
            *i += 2;
            true
        }
        "--no-bits" => {
            cfg.bits.enabled = false;
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
        .unwrap_or_else(|| die("位宽列表形如 8bit,16bit"))
}
