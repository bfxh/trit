//! 报告小件：决策比率。

use trit_learn::json::Json;

pub fn rates_json(decisions: &[usize; 3], records: usize) -> Json {
    let n = records.max(1) as f64;
    Json::obj()
        .put("accept", decisions[0] as f64 / n)
        .put("reject", decisions[1] as f64 / n)
        .put("defer", decisions[2] as f64 / n)
}
