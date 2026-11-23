//! `trit-data label`：把 gate 判定导出成人工/LLM judge 能填的标注包，再收回对拍。
//!
//! 两个模式：
//! - `label --jsonl <records> --sample 500 --out <dir>`：跑同一质量门，按
//!   `decision:主原因` 分层抽样，写 `label.csv`（`human_decision` / `human_note` 留空）
//!   与 `label.report.json`（输入 sha256、抽样规则、每层 available/sampled）。
//! - `label --apply <label.csv> --out <dir>`：校验人工列只取 accept/defer/reject，
//!   写 `model.jsonl` 与 `human.jsonl`（compare 直接吃），并给出一致率。
//!
//! 抽样确定性：种子 `--seed` + 层名 FNV 混合；同输入同种子逐行可复现。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use trit_core::fingerprint::file_sha256;
use trit_data::gate::{Config, evaluate};
use trit_data::schema::Trajectory;
use trit_data::script::trajectory_text;
use trit_learn::json::{Json, write_json};

use super::{created_unix, die, usage, write_lines};

pub const SEED_DEFAULT: u64 = 0x9e37_79b9_7f4a_7c15;
const VALID: [&str; 3] = ["accept", "defer", "reject"];

#[path = "label_csv.rs"]
mod label_csv;

struct Labeled {
    id: usize,
    task_id: String,
    decision: String,
    reasons: Vec<String>,
    text: String,
}

pub fn run(argv: &[String]) {
    let mut jsonl: Option<PathBuf> = None;
    let mut apply: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut sample = 500usize;
    let mut seed = SEED_DEFAULT;
    let mut index = 0usize;
    while index < argv.len() {
        match argv[index].as_str() {
            "--jsonl" => {
                jsonl = argv.get(index + 1).map(PathBuf::from);
                index += 2;
            }
            "--apply" => {
                apply = argv.get(index + 1).map(PathBuf::from);
                index += 2;
            }
            "--out" => {
                out = argv.get(index + 1).map(PathBuf::from);
                index += 2;
            }
            "--sample" => {
                sample = argv
                    .get(index + 1)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| die("--sample 需要正整数"));
                index += 2;
            }
            "--seed" => {
                seed = argv
                    .get(index + 1)
                    .and_then(|raw| parse_seed(raw))
                    .unwrap_or_else(|| die("--seed 需要十进制或 0x 十六进制"));
                index += 2;
            }
            other => die(&format!("未知参数 {other}")),
        }
    }
    let out = out.unwrap_or_else(|| PathBuf::from("label-pack"));
    match (jsonl, apply) {
        (Some(_), Some(_)) => die("--jsonl 与 --apply 只能二选一"),
        (Some(path), None) => sample_run(&path, sample, seed, &out),
        (None, Some(path)) => apply_run(&path, &out),
        (None, None) => usage(),
    }
}

fn parse_seed(raw: &str) -> Option<u64> {
    match raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => raw.parse().ok(),
    }
}

// ---------------------------------------------------------------- 抽样导出

fn sample_run(path: &Path, sample: usize, seed: u64, out: &Path) {
    let text = super::shared::input::read_text(path).unwrap_or_else(|e| die(&e));
    let input_sha = file_sha256(path).unwrap_or_default();
    let cfg = Config::default();
    let mut seen = std::collections::BTreeSet::new();
    let mut entries: Vec<Labeled> = Vec::new();
    let mut parse_errors = 0usize;

    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let id = entries.len();
        match Json::parse(line).and_then(|json| Trajectory::from_json(&json)) {
            Ok(trajectory) => {
                let value = evaluate(&trajectory, &cfg, &mut seen);
                entries.push(Labeled {
                    id,
                    task_id: trajectory.task_id.clone(),
                    decision: value.decision.as_str().to_string(),
                    reasons: value.reasons.iter().map(|r| (*r).to_string()).collect(),
                    text: trajectory_text(&trajectory),
                });
            }
            Err(_) => {
                parse_errors += 1;
                entries.push(Labeled {
                    id,
                    task_id: String::new(),
                    decision: "reject".to_string(),
                    reasons: vec!["parse_error".to_string()],
                    text: line.to_string(),
                });
            }
        }
    }
    if entries.is_empty() {
        die("输入里没有可标注记录");
    }

    let mut strata: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for entry in &entries {
        strata.entry(stratum(entry)).or_default().push(entry.id);
    }
    let quotas = quotas(&strata, sample);
    let mut selected: Vec<usize> = Vec::new();
    for (key, ids) in &strata {
        let mut ids = ids.clone();
        shuffle(&mut ids, mix(seed, key));
        let take = quotas.get(key).copied().unwrap_or(0).min(ids.len());
        selected.extend(ids.into_iter().take(take));
    }
    selected.sort_unstable();

    std::fs::create_dir_all(out).unwrap_or_else(|e| die(&format!("建不了 {}: {e}", out.display())));
    let rows: Vec<Vec<String>> = selected.iter().map(|id| row_of(&entries[*id])).collect();
    let csv = label_csv::encode(
        &[
            "id",
            "task_id",
            "model_decision",
            "reasons",
            "human_decision",
            "human_note",
            "text",
        ],
        &rows,
    );
    std::fs::write(out.join("label.csv"), csv).unwrap_or_else(|e| die(&e.to_string()));

    let mut strata_json = Json::obj();
    for (key, ids) in &strata {
        strata_json = strata_json.put(
            key.clone(),
            Json::obj()
                .put("available", ids.len() as u64)
                .put("sampled", quotas.get(key).copied().unwrap_or(0) as u64),
        );
    }
    let report = Json::obj()
        .put("schema", 1u32)
        .put("kind", "trit-data-label")
        .put("created_unix", created_unix())
        .put(
            "input",
            Json::obj()
                .put("path", path.display().to_string())
                .put("sha256", input_sha)
                .put("records", entries.len() as u64)
                .put("parse_errors", parse_errors as u64),
        )
        .put(
            "config",
            Json::obj()
                .put("sample", sample as u64)
                .put("seed", format!("0x{seed:016x}"))
                .put(
                    "rule",
                    "按 `decision:主原因` 分层；配额按比例 + 最大余数（每层不超过可用数）；\
                     同层内按 `seed ^ FNV(层名)` 洗牌取前 k；输出按 id 升序",
                ),
        )
        .put("strata", strata_json)
        .put(
            "counts",
            Json::obj()
                .put("records", entries.len() as u64)
                .put("sampled", selected.len() as u64)
                .put("strata", strata.len() as u64),
        );
    let path_report = out.join("label.report.json");
    write_json(&path_report, &report).unwrap_or_else(|e| die(&e));

    println!(
        "标注包：{} 条 → 抽 {} 条（{} 层）；csv {}",
        entries.len(),
        selected.len(),
        strata.len(),
        out.join("label.csv").display()
    );
    for (key, ids) in &strata {
        println!(
            "  {key}: 可用 {} / 抽 {}",
            ids.len(),
            quotas.get(key).copied().unwrap_or(0)
        );
    }
}

fn stratum(entry: &Labeled) -> String {
    match entry.reasons.first() {
        Some(reason) => format!("{}:{reason}", entry.decision),
        None => entry.decision.clone(),
    }
}

fn row_of(entry: &Labeled) -> Vec<String> {
    vec![
        entry.id.to_string(),
        entry.task_id.clone(),
        entry.decision.clone(),
        entry.reasons.join("|"),
        String::new(),
        String::new(),
        entry.text.clone(),
    ]
}

/// 比例配额 + 最大余数；每层不超过可用数，余量按「还能加」的层循环补。
fn quotas(strata: &BTreeMap<String, Vec<usize>>, sample: usize) -> BTreeMap<String, usize> {
    let total: usize = strata.values().map(Vec::len).sum();
    let mut out = BTreeMap::new();
    if total <= sample {
        for (key, ids) in strata {
            out.insert(key.clone(), ids.len());
        }
        return out;
    }
    let mut remainders: Vec<(u64, String)> = Vec::new();
    let mut used = 0usize;
    for (key, ids) in strata {
        let exact = sample as f64 * ids.len() as f64 / total as f64;
        let floor = exact.floor() as usize;
        let quota = floor.min(ids.len());
        used += quota;
        out.insert(key.clone(), quota);
        let fraction = ((exact - exact.floor()) * 1_000_000.0).round() as u64;
        remainders.push((fraction, key.clone()));
    }
    remainders.sort_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(&right.1)));
    let mut progress = true;
    while used < sample && progress {
        progress = false;
        for (_, key) in &remainders {
            if used >= sample {
                break;
            }
            let available = strata[key].len();
            if let Some(quota) = out.get_mut(key)
                && *quota < available
            {
                *quota += 1;
                used += 1;
                progress = true;
            }
        }
    }
    out
}

fn mix(seed: u64, key: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in key.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    seed ^ hash
}

fn shuffle(ids: &mut [usize], mut state: u64) {
    if state == 0 {
        state = 1;
    }
    for index in (1..ids.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let pick = (state % (index as u64 + 1)) as usize;
        ids.swap(index, pick);
    }
}

// ---------------------------------------------------------------- 收回对拍

fn apply_run(csv_path: &Path, out: &Path) {
    let text = std::fs::read_to_string(csv_path)
        .unwrap_or_else(|e| die(&format!("读不了 {}: {e}", csv_path.display())));
    let (headers, rows) = label_csv::decode(&text)
        .unwrap_or_else(|e| die(&format!("{}: {e}", csv_path.display())));
    let column = |name: &str| {
        headers
            .iter()
            .position(|header| header == name)
            .unwrap_or_else(|| die(&format!("{} 缺列 {name}", csv_path.display())))
    };
    let id_col = column("id");
    let model_col = column("model_decision");
    let human_col = column("human_decision");

    let mut model_lines = Vec::new();
    let mut human_lines = Vec::new();
    let mut mismatches = Vec::new();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut missing = 0usize;
    for (row_index, row) in rows.iter().enumerate() {
        let id: u64 = row
            .get(id_col)
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or_else(|| die(&format!("第 {} 行 id 不是整数", row_index + 2)));
        let model = row.get(model_col).map(|v| v.trim()).unwrap_or_default();
        let human = row.get(human_col).map(|v| v.trim()).unwrap_or_default();
        if !VALID.contains(&model) {
            die(&format!("第 {} 行 model_decision 非法：{model:?}", row_index + 2));
        }
        if human.is_empty() {
            missing += 1;
            continue;
        }
        if !VALID.contains(&human) {
            die(&format!(
                "第 {} 行 human_decision 只能是 accept/defer/reject，实际 {human:?}",
                row_index + 2
            ));
        }
        *counts.entry(human.to_string()).or_insert(0) += 1;
        if human != model {
            mismatches.push(id);
        }
        model_lines.push(Json::obj().put("id", id).put("decision", model).dump());
        human_lines.push(Json::obj().put("id", id).put("decision", human).dump());
    }
    if missing > 0 {
        die(&format!(
            "还有 {missing} 行没填 human_decision；填完再 --apply（不接受空值当默认）"
        ));
    }
    if human_lines.is_empty() {
        die("没有可对拍的标注行");
    }

    std::fs::create_dir_all(out).unwrap_or_else(|e| die(&format!("建不了 {}: {e}", out.display())));
    write_lines(&out.join("model.jsonl"), &model_lines);
    write_lines(&out.join("human.jsonl"), &human_lines);
    let n = human_lines.len();
    let agreement = (n - mismatches.len()) as f64 / n as f64;
    let report = Json::obj()
        .put("schema", 1u32)
        .put("kind", "trit-data-label-apply")
        .put("created_unix", created_unix())
        .put(
            "input",
            Json::obj()
                .put("path", csv_path.display().to_string())
                .put("sha256", file_sha256(csv_path).unwrap_or_default())
                .put("rows", rows.len() as u64)
                .put("skipped_empty", missing as u64),
        )
        .put(
            "counts",
            Json::obj()
                .put("n", n as u64)
                .put("agreement", agreement)
                .put("mismatches", mismatches.len() as u64),
        )
        .put(
            "human_decisions",
            counts.iter().fold(Json::obj(), |json, (key, value)| {
                json.put(key.clone(), *value as u64)
            }),
        );
    let report_path = out.join("label.apply.json");
    write_json(&report_path, &report).unwrap_or_else(|e| die(&e));
    println!(
        "标注对拍：n {n}，一致率 {:.4}（不一致 {}）；{} 与 {}",
        agreement,
        mismatches.len(),
        out.join("model.jsonl").display(),
        out.join("human.jsonl").display()
    );
    println!(
        "下一步：trit-data compare --a {} --b {}",
        out.join("model.jsonl").display(),
        out.join("human.jsonl").display()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strata_fixture() -> BTreeMap<String, Vec<usize>> {
        let mut strata = BTreeMap::new();
        strata.insert("accept".to_string(), (0..60).collect());
        strata.insert("defer:language".to_string(), (60..90).collect());
        strata.insert("reject:no_evidence".to_string(), (90..100).collect());
        strata
    }

    #[test]
    fn quotas_never_exceed_available_and_sum_to_sample() {
        let strata = strata_fixture();
        let quotas = quotas(&strata, 50);
        let total: usize = quotas.values().sum();
        assert_eq!(total, 50);
        for (key, ids) in &strata {
            assert!(quotas[key] <= ids.len(), "{key}");
        }
        // 分层按比例：60/100 应拿 30，10/100 至少 1。
        assert_eq!(quotas["accept"], 30);
        assert!(quotas["reject:no_evidence"] >= 1);
    }

    #[test]
    fn quotas_take_everything_when_sample_exceeds_records() {
        let strata = strata_fixture();
        let quotas = quotas(&strata, 500);
        assert_eq!(quotas.values().sum::<usize>(), 100);
    }

    #[test]
    fn sampling_is_deterministic_and_order_stable() {
        let strata = strata_fixture();
        let quotas = quotas(&strata, 30);
        let first = pick(&strata, &quotas, 0x1234);
        let second = pick(&strata, &quotas, 0x1234);
        assert_eq!(first, second);
        let other = pick(&strata, &quotas, 0x9999);
        assert_ne!(first, other);
    }

    fn pick(
        strata: &BTreeMap<String, Vec<usize>>,
        quotas: &BTreeMap<String, usize>,
        seed: u64,
    ) -> Vec<usize> {
        let mut selected = Vec::new();
        for (key, ids) in strata {
            let mut ids = ids.clone();
            shuffle(&mut ids, mix(seed, key));
            selected.extend(ids.into_iter().take(quotas[key]));
        }
        selected.sort_unstable();
        selected
    }
}
