use crate::commands::die;
use trit_data::content::ContentFilter;

#[path = "format_opts.rs"]
mod format_opts;
#[path = "label_opts.rs"]
mod label_opts;
#[path = "quality_opts.rs"]
mod quality_opts;

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
        _ => {
            label_opts::apply(filter, flag, argv, i)
                || quality_opts::apply(filter, flag, argv, i)
                || format_opts::apply(filter, flag, argv, i)
        }
    }
}

fn set(v: &mut bool, x: bool, i: &mut usize) -> bool {
    *v = x;
    *i += 1;
    true
}
