use crate::commands::die;
use trit_data::content::ContentFilter;

#[path = "lang_opts_content.rs"]
mod lang_opts_content;

pub fn apply(filter: &mut ContentFilter, flag: &str, argv: &[String], i: &mut usize) -> bool {
    match flag {
        "--no-language" => {
            filter.lang.enabled = false;
            *i += 1;
            true
        }
        "--lang-deny" => {
            filter.lang.deny = list(argv, *i);
            *i += 2;
            true
        }
        "--lang-allow" => {
            filter.lang.allow = Some(list(argv, *i));
            *i += 2;
            true
        }
        _ => lang_opts_content::apply(filter, flag, argv, i),
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
        .unwrap_or_else(|| die("语言列表形如 zh,en"))
}
