//! repo 控制台摘要：扫描统计 + 可选依赖闭包。

use std::path::Path;

use trit_data::repo;
use trit_data::repo::Unit;

use super::{repo_json, repo_stats};

#[allow(clippy::too_many_arguments)]
pub fn summary(
    root: &Path,
    units: &[Unit],
    s: &repo_stats::Stats,
    d: &repo::deps::Summary,
    dup: &repo::dup::Dup,
    closure: Option<&str>,
    max_files: usize,
) -> String {
    let mut out = format!(
        "仓库 {}：文件 {}（生成 {}，测试 {}，依赖边 {}，测试关联 {}，环上 {}，重复 {}/{}）",
        root.display(),
        units.len(),
        s.generated,
        s.tests,
        d.edges,
        d.test_links,
        d.cyclic,
        dup.exact.len(),
        dup.near.len()
    );
    if let Some(start) = closure {
        let c = repo_json::closure_json(units, start, max_files);
        out.push_str(&format!("
闭包 {start}：{} 个文件", c.dump()));
    }
    out
}
