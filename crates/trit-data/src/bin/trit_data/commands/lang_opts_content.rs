use crate::commands::die;
#[path = "quality_opts.rs"]
mod quality_opts;
use trit_data::content::ContentFilter;
#[path = "label_opts.rs"]
mod label_opts;
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
        _ => label_opts::apply(filter, flag, argv, i) || quality_opts::apply(filter, flag, argv, i),
    }
}
