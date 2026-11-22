//! 代码 / 自然语言基础参数。

use trit_data::content::ContentFilter;

use crate::commands::die;

pub fn apply(filter: &mut ContentFilter, flag: &str, argv: &[String], i: &mut usize) -> bool {
    match flag {
        "--code" => set(&mut filter.code, true, i),
        "--no-code" => set(&mut filter.code, false, i),
        "--natural" => set(&mut filter.natural, true, i),
        "--no-natural" => set(&mut filter.natural, false, i),
        "--code-threshold" => {
            filter.threshold = argv
                .get(*i + 1)
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| die("--code-threshold 需要小数"));
            *i += 2;
            true
        }
        _ => false,
    }
}

fn set(v: &mut bool, x: bool, i: &mut usize) -> bool {
    *v = x;
    *i += 1;
    true
}
