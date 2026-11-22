//! 报告用 JSON 小件。

use std::collections::BTreeMap;

use trit_data::gate::Verdict;
use trit_learn::json::Json;

pub fn gate_json(v: &Verdict) -> Json {
    Json::obj().put("decision", v.decision.as_str()).put(
        "reasons",
        Json::arr(v.reasons.iter().map(|r| Json::str(*r)).collect()),
    )
}

pub fn reasons_json(m: &BTreeMap<&'static str, usize>) -> Json {
    let mut out = Json::obj();
    for (k, v) in m {
        out = out.put(*k, *v as u64);
    }
    out
}
