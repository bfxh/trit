//! 计数 map → JSON。

use std::collections::BTreeMap;

use trit_learn::json::Json;

pub fn map_json(m: &BTreeMap<String, usize>) -> Json {
    let mut out = Json::obj();
    for (k, v) in m {
        out = out.put(k.clone(), *v as u64);
    }
    out
}
