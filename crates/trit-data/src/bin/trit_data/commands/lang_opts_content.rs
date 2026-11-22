//! 内容过滤参数（code / natural / threshold）。

use trit_data::content::ContentFilter;

use crate::commands::die;

pub fn apply(filter: &mut ContentFilter, flag: &str, argv: &[String], i: &mut usize) -> bool {
    match flag {
        "--code" => {
            filter.code = true;
            *i += 1;
            true
        }
        "--no-code" => {
            filter.code = false;
            *i += 1;
            true
        }
        "--natural" => {
            filter.natural = true;
            *i += 1;
            true
        }
        "--no-natural" => {
            filter.natural = false;
            *i += 1;
            true
        }
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
