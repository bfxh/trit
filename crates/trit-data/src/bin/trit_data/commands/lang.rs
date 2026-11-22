//! `lang` 子命令：单次/批量主语言检测与过滤。

use std::collections::BTreeMap;
use std::path::PathBuf;

use trit_data::schema::Trajectory;
use trit_data::script::{primary_language, script_mix, trajectory_text};
use trit_learn::json::{Json, write_json};

use super::{created_unix, die, usage, write_lines};

pub fn run(argv: &[String]) {
    let mut text: Option<String> = None;
    let mut jsonl: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut deny: Vec<String> = Vec::new();
    let mut allow: Option<Vec<String>> = None;
    let mut off = false;
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--text" => {
                text = argv.get(i + 1).cloned();
                i += 2;
            }
            "--jsonl" => {
                jsonl = argv.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--out" => {
                out = argv.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--deny" => {
                deny = list(argv, i);
                i += 2;
            }
            "--allow" => {
                allow = Some(list(argv, i));
                i += 2;
            }
            "--off" => {
                off = true;
                i += 1;
            }
            other => die(&format!("未知参数 {other}")),
        }
    }
    if off {
        println!("语言过滤：off");
        return;
    }
    let denied = |lang: &str| {
        deny.iter().any(|x| x == lang)
            || allow.as_ref().is_some_and(|a| !a.iter().any(|x| x == lang))
    };
    if let Some(t) = text {
        let lang = primary_language(&t);
        let m = script_mix(&t);
        println!(
            "{}",
            Json::obj()
                .put("lang", lang)
                .put("denied", denied(lang))
                .put(
                    "mix",
                    Json::obj()
                        .put("han", m.han)
                        .put("latin", m.latin)
                        .put("kana", m.kana)
                        .put("hangul", m.hangul)
                        .put("cyrillic", m.cyrillic)
                        .put("arabic", m.arabic)
                        .put("other", m.other)
                )
                .dump()
        );
        return;
    }
    let jsonl = jsonl.unwrap_or_else(|| usage());
    let raw = std::fs::read_to_string(&jsonl)
        .unwrap_or_else(|e| die(&format!("读不了 {}: {e}", jsonl.display())));
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut lines = Vec::new();
    let mut flagged = 0usize;
    for line in raw.lines().filter(|l| !l.trim().is_empty()) {
        let j = Json::parse(line).unwrap_or_else(|e| die(&format!("{}: {e}", jsonl.display())));
        let t =
            Trajectory::from_json(&j).unwrap_or_else(|e| die(&format!("{}: {e}", jsonl.display())));
        let lang = primary_language(&trajectory_text(&t)).to_string();
        *counts.entry(lang.clone()).or_insert(0) += 1;
        let d = denied(&lang);
        if d {
            flagged += 1;
        }
        lines.push(
            Json::obj()
                .put("task_id", t.task_id)
                .put("lang", lang)
                .put("denied", d)
                .dump(),
        );
    }
    let dist: Vec<String> = counts.iter().map(|(k, v)| format!("{k}={v}")).collect();
    println!(
        "记录 {}：被过滤 {}；分布 {}",
        lines.len(),
        flagged,
        dist.join(", ")
    );
    if let Some(dir) = out {
        std::fs::create_dir_all(&dir)
            .unwrap_or_else(|e| die(&format!("建不了 {}: {e}", dir.display())));
        write_lines(&dir.join("languages.jsonl"), &lines);
        let mut distribution = Json::obj();
        for (k, v) in &counts {
            distribution = distribution.put(k.clone(), *v as u64);
        }
        let report = Json::obj()
            .put("schema", 1u32)
            .put("kind", "trit-data-lang")
            .put("created_unix", created_unix())
            .put(
                "input",
                Json::obj().put("path", jsonl.display().to_string()),
            )
            .put("records", lines.len() as u64)
            .put("filtered", flagged as u64)
            .put("distribution", distribution);
        write_json(&dir.join("languages.report.json"), &report).unwrap_or_else(|e| die(&e));
    }
}

fn list(argv: &[String], i: usize) -> Vec<String> {
    argv.get(i + 1)
        .map(|s| {
            s.split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect()
        })
        .unwrap_or_else(|| die("语言列表形如 zh,ja"))
}
