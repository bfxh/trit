//! `TRIT_BUDGET` 预算图：把预算编译器产出的 per-matrix bits 读进加载器。
//!
//! 只认 `blocks[].layer/kind/bits`；`actual_bytes` 留给规划器和报告核对。

use serde_json::Value;
use std::collections::HashMap;
use std::fs;

pub(crate) struct Budget {
    bits: HashMap<String, u32>,
}

impl Budget {
    pub(crate) fn from_env() -> Option<Self> {
        let path = std::env::var("TRIT_BUDGET").ok()?;
        let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("读 {path} 失败：{e}"));
        let value: Value =
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("解析 {path} 失败：{e}"));
        let blocks = value["blocks"]
            .as_array()
            .unwrap_or_else(|| panic!("{path} 缺少 blocks"));
        let mut bits = HashMap::new();
        for block in blocks {
            let layer = block["layer"].as_u64().expect("budget layer") as usize;
            let kind = block["kind"].as_str().expect("budget kind");
            let value = block["bits"].as_u64().expect("budget bits") as u32;
            assert!((2..=5).contains(&value), "budget bits 要在 2..=5");
            let short = format!("{}{layer}", code(kind));
            bits.insert(short, value);
        }
        Some(Self { bits })
    }

    pub(crate) fn bits(&self, short: &str) -> Option<u32> {
        self.bits.get(short).copied()
    }
}

fn code(kind: &str) -> &'static str {
    match kind {
        "attention.self.query" => "q",
        "attention.self.key" => "k",
        "attention.self.value" => "v",
        "attention.output.dense" => "o",
        "intermediate.dense" => "i",
        "output.dense" => "f",
        other => panic!("未知 budget kind：{other}"),
    }
}
