//! 一把 token 尺，CLI 与 MCP 共用。
//!
//! 为什么要抽出来：读数上限这件事一旦有两把尺（一处按字符、一处按字节），
//! 智能体拿到的"预算"就和实际灌进上下文的量脱钩了——它会以为还有空间。
//!
//! 口径（与 MCP v0 的实测同源）：ASCII 每 4 字符记 1 元，非 ASCII 每字符按 4 字节估、
//! 每 4 字节记 1 元 ⇒ 中文约 1 字 = 1 元。这是估算，不是分词器读数。

use serde_json::{Map, Value};

/// 估算一段文本的 token 元数（见模块口径）。
pub fn estimate(text: &str) -> u64 {
    let mut units = 0u64;
    for ch in text.chars() {
        units += if ch.is_ascii() { 1 } else { 4 };
    }
    units.div_ceil(4)
}

/// 把一个读数压进 `budget` 元：从后往前丢数组里的行，并把丢了多少写在输出里。
///
/// 只丢**完整的行**，绝不改写任何一行的内容；丢完还超就照原样返回（超了多少看得见，
/// 比"静默截半条 JSON"诚实）。返回丢掉的行数。
pub fn cap_rows(value: &mut Value, budget: u64) -> usize {
    // 顶层是数组（`log --json`）：没地方写 truncation ⇒ 只丢行，由调用方把丢了多少喊出来。
    if value.is_array() {
        let mut dropped = 0usize;
        while let Some(rows) = value.as_array_mut() {
            if rows.is_empty()
                || estimate(&serde_json::to_string(&*rows).unwrap_or_default()) <= budget
            {
                break;
            }
            rows.pop();
            dropped += 1;
        }
        return dropped;
    }
    if !value.is_object() {
        return 0;
    }
    let mut dropped = 0usize;
    // 没超就原样交出去：元数据本身也占上下文，一条不丢还挂个字段等于白送几元。
    if estimate(&value.to_string()) <= budget {
        return 0;
    }
    // 要丢行时，`truncation` 自己也占上下文 ⇒ 挂上它再判超不超，否则"压到预算内"是假的。
    write_budget(value, budget, dropped);
    while estimate(&value.to_string()) > budget {
        let Some(key) = longest_array(value) else {
            break;
        };
        let Some(rows) = value.get_mut(&key).and_then(Value::as_array_mut) else {
            break;
        };
        if rows.pop().is_none() {
            break;
        }
        dropped += 1;
        write_budget(value, budget, dropped);
    }
    dropped
}

/// 选最长的那个数组下手：读数的主列表通常就是它，先丢短列表等于没压。
fn longest_array(value: &Value) -> Option<String> {
    let map = value.as_object()?;
    map.iter()
        .filter_map(|(key, field)| {
            field
                .as_array()
                .filter(|rows| !rows.is_empty())
                .map(|rows| (key.clone(), rows.len()))
        })
        .max_by_key(|(_, len)| *len)
        .map(|(key, _)| key)
}

/// 把预算与丢弃数写进读数本身（智能体据此决定要不要翻页）。
fn write_budget(value: &mut Value, budget: u64, dropped: usize) {
    let Value::Object(fields) = value else {
        return;
    };
    let mut meta = Map::new();
    meta.insert("budget".to_string(), Value::from(budget));
    meta.insert("dropped".to_string(), Value::from(dropped as u64));
    // 指引不塞进每个载荷（那条中文要 ~40 元， tight 预算下它自己就超预算）：
    // 数组型读数由 CLI 在 stderr 喊，对象型读数看 dropped 自己决定怎么续读。
    fields.insert("truncation".to_string(), Value::Object(meta));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_and_cjk_share_one_ruler() {
        assert_eq!(estimate("abcd"), 1);
        assert_eq!(estimate("abcdefgh"), 2);
        assert_eq!(estimate("中文"), 2);
        assert!(estimate("混合 mixed") >= 2);
    }

    #[test]
    fn budget_big_enough_drops_nothing_and_says_so() {
        let mut value = serde_json::json!({"changes": [{"path": "a"}, {"path": "b"}]});
        assert_eq!(cap_rows(&mut value, 4_000), 0);
        assert_eq!(value["changes"].as_array().unwrap().len(), 2);
        assert!(
            value.get("truncation").is_none(),
            "一条都没丢就不该挂元数据"
        );
    }

    #[test]
    fn over_budget_trims_tail_rows_and_reports_the_count() {
        let rows: Vec<_> = (0..80)
            .map(|i| {
                serde_json::json!({
                    "id": format!("ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff{i:02x}"),
                    "message": format!("提交 {i}：把这块逻辑挪到别处去，说明写得很长一点"),
                })
            })
            .collect();
        let mut value = serde_json::json!({"rows": rows});
        let budget = 400u64;
        let dropped = cap_rows(&mut value, budget);
        let kept = value["rows"].as_array().unwrap().len();
        assert!(dropped > 0, "80 行长读数压不进 400 元");
        assert_eq!(dropped + kept, 80, "只丢行，不改行");
        assert!(
            estimate(&value.to_string()) <= budget,
            "压完必须真的在预算内：{} > {budget}",
            estimate(&value.to_string())
        );
        assert_eq!(value["truncation"]["dropped"], dropped);
        assert_eq!(value["truncation"]["budget"], 400);
    }

    #[test]
    fn exactly_at_budget_never_trims_and_never_pays_for_metadata() {
        // 这条钉的是"预算是上限不是目标"：刚好等于读数大小时一条都不该丢，
        // 也不该挂 truncation（元数据本身要占上下文，挂了反而超预算——那就是假达标）。
        let rows: Vec<_> = (0..6)
            .map(|i| serde_json::json!({"n": i, "pad": "x".repeat(60)}))
            .collect();
        let mut value = serde_json::json!({"rows": rows});
        let exact = estimate(&value.to_string());
        assert_eq!(cap_rows(&mut value, exact), 0);
        assert_eq!(value["rows"].as_array().unwrap().len(), 6);
        assert!(value.get("truncation").is_none());

        // 退一元就必须真丢，并且连元数据一起算仍在预算内
        let mut value = serde_json::json!({"rows": rows});
        let dropped = cap_rows(&mut value, exact - 1);
        assert!(dropped >= 1, "退一元也得丢行，否则'压进预算'是假的");
        assert!(
            estimate(&value.to_string()) < exact,
            "压完必须连元数据一起算仍在预算内：{}",
            estimate(&value.to_string())
        );
    }

    #[test]
    fn tail_rows_are_dropped_not_the_head_ones() {
        let mut value = serde_json::json!({
            "rows": [{"n": 0, "pad": "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"},
                     {"n": 1, "pad": "yyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyy"},
                     {"n": 2, "pad": "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"}],
        });
        let full = estimate(&value.to_string());
        let per_row = estimate(&serde_json::to_string(&value["rows"][0]).unwrap_or_default());
        let dropped = cap_rows(&mut value, full - per_row);
        assert!(dropped >= 1, "退一档预算必须至少丢一行");
        let kept = value["rows"].as_array().unwrap();
        assert_eq!(kept.len(), 3 - dropped, "丢多少就该少多少");
        for (index, row) in kept.iter().enumerate() {
            assert_eq!(row["n"], index, "只能从尾部丢，不能丢最新的");
        }
        assert!(estimate(&value.to_string()) <= full - per_row);
    }
}
