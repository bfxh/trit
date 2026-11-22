//! 数据卡字段覆盖率。

use std::collections::BTreeMap;

use trit_data::schema::Trajectory;
use trit_data::script::trajectory_text;
use trit_data::{bits, format, semantics, style};
use trit_learn::json::Json;

#[derive(Default)]
pub struct Coverage {
    records: usize,
    present: BTreeMap<&'static str, usize>,
}

impl Coverage {
    pub fn add(&mut self, t: &Trajectory) {
        self.records += 1;
        let text = trajectory_text(t);
        let mut mark = |k: &'static str, ok: bool| {
            if ok {
                *self.present.entry(k).or_insert(0) += 1;
            }
        };
        mark("source", t.source.is_some());
        mark("license", t.license.is_some());
        mark("labels", !t.labels.is_empty());
        mark("style_non_other", style::detect(&text).as_str() != "other");
        mark(
            "semantics_non_other",
            semantics::detect(&text).as_str() != "other",
        );
        mark("bits", !bits::labels(&text).is_empty());
        mark("formats", !format::detect(&text).is_empty());
    }

    pub fn json(&self) -> Json {
        let n = self.records.max(1) as f64;
        let mut out = Json::obj().put("records", self.records as u64);
        for k in [
            "source",
            "license",
            "labels",
            "style_non_other",
            "semantics_non_other",
            "bits",
            "formats",
        ] {
            let c = self.present.get(k).copied().unwrap_or(0) as f64 / n;
            out = out.put(k, c);
        }
        out
    }
}
