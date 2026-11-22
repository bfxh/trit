//! 位宽 / 多值筛选参数（decontaminate / mix）。

use trit_data::content::ContentFilter;

use crate::commands::die;

pub fn apply(filter: &mut ContentFilter, flag: &str, argv: &[String], i: &mut usize) -> bool {
    match flag {
        "--bits" => {
            filter.bits.enabled = true;
            filter.bits.allow = Some(list(argv, *i));
            *i += 2;
            true
        }
        "--bits-deny" => {
            filter.bits.enabled = true;
            filter.bits.deny = list(argv, *i);
            *i += 2;
            true
        }
        "--no-bits" => {
            filter.bits.enabled = false;
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
