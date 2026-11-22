//! 内容标签（风格 / 语义）参数。

use trit_data::content::ContentFilter;

use crate::commands::die;

pub fn apply(filter: &mut ContentFilter, flag: &str, argv: &[String], i: &mut usize) -> bool {
    match flag {
        "--style" => {
            filter.style.enabled = true;
            filter.style.allow = Some(list(argv, *i));
            *i += 2;
            true
        }
        "--style-deny" => {
            filter.style.enabled = true;
            filter.style.deny = list(argv, *i);
            *i += 2;
            true
        }
        "--no-style" => {
            filter.style.enabled = false;
            *i += 1;
            true
        }
        "--semantics" => {
            filter.semantics.enabled = true;
            filter.semantics.allow = Some(list(argv, *i));
            *i += 2;
            true
        }
        "--semantics-deny" => {
            filter.semantics.enabled = true;
            filter.semantics.deny = list(argv, *i);
            *i += 2;
            true
        }
        "--no-semantics" => {
            filter.semantics.enabled = false;
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
        .unwrap_or_else(|| die("标签列表形如 formal,tutorial"))
}
