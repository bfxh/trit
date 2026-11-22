//! 内容代码质量 / 教育性开关。

use trit_data::content::ContentFilter;

use crate::commands::die;

pub fn apply(filter: &mut ContentFilter, flag: &str, argv: &[String], i: &mut usize) -> bool {
    match flag {
        "--code-quality" => {
            filter.code_quality = true;
            *i += 1;
            true
        }
        "--no-code-quality" => {
            filter.code_quality = false;
            *i += 1;
            true
        }
        "--edu-min" => {
            filter.edu_min = argv
                .get(*i + 1)
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| die("--edu-min 需要小数"));
            *i += 2;
            true
        }
        _ => false,
    }
}
