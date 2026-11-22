//! 去重报告：精确重复组与近似重复组。

use trit_data::repo::dup::{Dup, ExactGroup, NearGroup};
use trit_learn::json::Json;

pub fn json(d: &Dup) -> Json {
    Json::obj()
        .put("files", d.files as u64)
        .put("skipped", d.skipped as u64)
        .put("exact_groups", Json::arr(d.exact.iter().map(exact).collect()))
        .put("near_groups", Json::arr(d.near.iter().map(near).collect()))
}

fn exact(g: &ExactGroup) -> Json {
    Json::obj()
        .put("sha256", g.hash.clone())
        .put("bytes", g.bytes)
        .put("paths", strings(&g.paths))
}

fn near(g: &NearGroup) -> Json {
    Json::obj()
        .put("similarity", g.similarity)
        .put("paths", strings(&g.paths))
}

fn strings(v: &[String]) -> Json {
    Json::arr(v.iter().map(Json::str).collect())
}