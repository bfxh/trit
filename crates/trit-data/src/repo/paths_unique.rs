//! 同目录唯一候选：测试文件去掉标记后找不到同名源文件时，若同目录只有一个
//! 同扩展名的非测试源文件，就连它。确定性规则，不做名字相似度猜测。

use std::collections::BTreeSet;

use super::marks::{is_source, is_test_path};
use super::paths::parent_dir;
use crate::repo::Unit;

pub fn unique_sibling(u: &Unit, paths: &BTreeSet<String>) -> Option<String> {
    if !u.is_test {
        return None;
    }
    let dir = parent_dir(&u.path);
    let head = format!("{dir}/");
    let ext = extension(&u.path);
    let mut hit: Option<&String> = None;
    for p in paths {
        if p == &u.path || !p.starts_with(&head) || p[head.len()..].contains('/') {
            continue;
        }
        if extension(p) != ext || is_test_path(std::path::Path::new(p)) {
            continue;
        }
        if !is_source(std::path::Path::new(p)) || hit.is_some() {
            return None;
        }
        hit = Some(p);
    }
    hit.cloned()
}

fn extension(p: &str) -> &str {
    p.rsplit('.').next().unwrap_or("")
}
