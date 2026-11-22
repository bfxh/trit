//! gate 的代码质量阈值参数。

use trit_data::gate::Config;

use crate::commands::die;

pub fn apply(cfg: &mut Config, flag: &str, argv: &[String], i: &mut usize) -> bool {
    let lim = &mut cfg.code_quality_limits;
    match flag {
        "--code-long" => {
            lim.max_long_line_ratio = num(argv, *i);
            *i += 2;
            true
        }
        "--code-alnum" => {
            lim.min_alnum_ratio = num(argv, *i);
            *i += 2;
            true
        }
        "--code-comment" => {
            lim.max_comment_ratio = num(argv, *i);
            *i += 2;
            true
        }
        "--code-repeat" => {
            lim.max_repeat_line_ratio = num(argv, *i);
            *i += 2;
            true
        }
        "--allow-auto-generated" => {
            lim.allow_auto_generated = true;
            *i += 1;
            true
        }
        _ => false,
    }
}

fn num(argv: &[String], i: usize) -> f64 {
    argv.get(i + 1)
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| die("阈值需要小数"))
}
