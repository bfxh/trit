//! 语言过滤参数解析（decontaminate / mix 共用）。

use trit_data::lang_filter::LangFilter;

use crate::commands::die;

pub fn apply(filter: &mut LangFilter, flag: &str, argv: &[String], i: &mut usize) -> bool {
    match flag {
        "--no-language" => {
            filter.enabled = false;
            *i += 1;
            true
        }
        "--lang-deny" => {
            filter.deny = list(argv, *i);
            *i += 2;
            true
        }
        "--lang-allow" => {
            filter.allow = Some(list(argv, *i));
            *i += 2;
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
        .unwrap_or_else(|| die("语言列表形如 zh,ja"))
}
