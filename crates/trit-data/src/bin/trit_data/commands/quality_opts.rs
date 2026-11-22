use crate::commands::die;
use trit_data::content::ContentFilter;

#[path = "limit_opts.rs"]
mod limit_opts;

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
        _ => limit_opts::apply(filter, flag, argv, i),
    }
}
