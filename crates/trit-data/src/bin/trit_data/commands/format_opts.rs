//! 格式筛选参数（decontaminate / mix）。

use trit_data::content::ContentFilter;

use crate::commands::die;

pub fn apply(filter: &mut ContentFilter, flag: &str, argv: &[String], i: &mut usize) -> bool {
    match flag {
        "--format" => {
            filter.formats.enabled = true;
            filter.formats.allow = Some(list(argv, *i));
            *i += 2;
            true
        }
        "--format-deny" => {
            filter.formats.enabled = true;
            filter.formats.deny = list(argv, *i);
            *i += 2;
            true
        }
        "--no-format" => {
            filter.formats.enabled = false;
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
