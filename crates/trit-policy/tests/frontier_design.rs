//! 前沿设计门禁（`docs/positioning/FRONTIER.md` 的机器可判部分）。
//!
//! 只判两件能机器判的事：默认件清单的字段完整性 + 路径存在；S 级任务包的四门槛覆盖。
//! 它判不了「设计是否真的更前沿」——那靠评审与负结果（宪章 §3）。

use std::path::{Path, PathBuf};

use serde_json::Value;

const REQUIRED_TOOL_FIELDS: &[&str] = &[
    "name",
    "kind",
    "path",
    "role",
    "replaces",
    "baseline",
    "receipt",
    "negative",
    "why_frontier",
];

/// 四门槛的候选词：任务包用词各不相同（"官方引擎差分"就是独立 oracle 的一种），
/// 门禁判"语义面有没有覆盖"，不做字面匹配。
const S_LEVEL_GATES: &[(&str, &[&str])] = &[
    (
        "独立 oracle",
        &[
            "独立 oracle",
            "oracle",
            "官方引擎差分",
            "官方实现",
            "第二实现",
            "穷举",
        ],
    ),
    ("性质", &["性质", "不变量"]),
    ("性能", &["性能", "吞吐", "预算"]),
    ("负结果", &["负结果", "负更新"]),
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/<crate> 之上应有仓库根")
        .to_path_buf()
}

/// 校验清单结构；返回所有违例（空 = 通过）。抽成函数是为了能对坏输入做反证。
fn validate_manifest(value: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    if value.get("schema").and_then(Value::as_u64) != Some(1) {
        problems.push("schema 必须为 1".to_owned());
    }
    let Some(tools) = value.get("tools").and_then(Value::as_array) else {
        problems.push("缺少 tools 数组".to_owned());
        return problems;
    };
    if tools.is_empty() {
        problems.push("tools 不能为空".to_owned());
    }
    for (index, tool) in tools.iter().enumerate() {
        for field in REQUIRED_TOOL_FIELDS {
            let ok = tool
                .get(*field)
                .and_then(Value::as_str)
                .is_some_and(|text| !text.trim().is_empty());
            if !ok {
                problems.push(format!("tools[{index}] 缺字段或为空：{field}"));
            }
        }
    }
    problems
}

fn manifest() -> Value {
    let path = repo_root().join("docs/specs/DEFAULT-TOOLS.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读不到 {}: {error}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("{} 不是合法 JSON: {error}", path.display()))
}

#[test]
fn default_tools_manifest_is_complete() {
    let problems = validate_manifest(&manifest());
    assert!(problems.is_empty(), "默认件清单有问题：{problems:?}");
}

#[test]
fn default_tool_paths_exist() {
    let root = repo_root();
    let value = manifest();
    for tool in value["tools"].as_array().expect("tools 数组") {
        let name = tool["name"].as_str().unwrap_or("<unnamed>");
        let path = tool["path"].as_str().expect("path");
        let absolute = root.join(path);
        assert!(
            absolute.exists(),
            "默认件 {name} 的路径不存在：{path}（解析为 {}）",
            absolute.display()
        );
    }
}

#[test]
fn s_level_task_packages_carry_four_gates() {
    let root = repo_root();
    let mut checked = 0usize;
    for dir in [root.join("docs/AGENT-TASKS"), root.join("docs/specs")] {
        for entry in std::fs::read_dir(&dir).expect("读任务包目录").flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let is_task =
                name.ends_with("-TASK.md") || (name.starts_with("S-") && name.ends_with(".md"));
            if !is_task {
                continue;
            }
            checked += 1;
            let text = std::fs::read_to_string(&path).expect("读任务包");
            let missing: Vec<&str> = S_LEVEL_GATES
                .iter()
                .filter(|(_, candidates)| {
                    !candidates.iter().any(|candidate| text.contains(candidate))
                })
                .map(|(label, _)| *label)
                .collect();
            assert!(
                missing.is_empty(),
                "S 级任务包 {name} 缺少四门槛中的：{missing:?}"
            );
        }
    }
    assert!(
        checked > 0,
        "docs/AGENT-TASKS 与 docs/specs 下没有任何任务包"
    );
}

#[test]
fn validator_rejects_incomplete_manifest() {
    // 反证：门禁必须真的会红，否则它只是文档。
    let bad = serde_json::json!({"schema": 1, "tools": [{"name": "only-name"}]});
    let problems = validate_manifest(&bad);
    assert!(!problems.is_empty(), "缺字段的清单必须被拒");
    assert!(
        problems.iter().any(|problem| problem.contains("replaces")),
        "应点名缺失字段：{problems:?}"
    );

    let wrong_schema = serde_json::json!({"schema": 2, "tools": []});
    let problems = validate_manifest(&wrong_schema);
    assert!(
        problems.iter().any(|problem| problem.contains("schema")),
        "{problems:?}"
    );
}
