//! CLI `propose` / `train-proposer` / `propose-learned`：学习式单点编辑提议。
//!
//! `propose` 保留原来的穷举/束搜索；`train-proposer` 用 `analyze` 的实测收益训练
//! 线性打分器；`propose-learned` 用模型打分，`--verify` 再走一次库内实测复核。

use std::fs;

use serde_json::{Value, json};

use crate::learned::{self, Scorer};
use crate::wire;

use super::{
    atomic_write, emit, err, load_net, load_oracle, no_double_stdin, one, opt_u64, opt_usize,
    parse, value,
};

pub(super) fn propose(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &["pairs"])?;
    let path = one(&parsed, "propose <artifact|-> --oracle <json|->")?;
    let oracle_path = value(&parsed, "oracle")?;
    no_double_stdin(&[path, oracle_path])?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let limit = opt_usize(&parsed, "limit", 16)?;
    let depth = match parsed.values.get("depth") {
        Some(raw) => raw
            .parse::<usize>()
            .map_err(|_| format!("--depth 需要 1/2/3，得到 `{raw}`"))?,
        None if parsed.switches.contains("pairs") => 2,
        None => 1,
    };
    match depth {
        1 => {
            let analysis = crate::analyze::analyze(&net, &oracle, limit).map_err(err)?;
            emit(&wire::analysis_to_value(&analysis))
        }
        2 => {
            let max_pairs = opt_usize(&parsed, "max-pairs", 20_000)?;
            let analysis =
                crate::analyze::analyze_pairs(&net, &oracle, limit, max_pairs).map_err(err)?;
            emit(&wire::pair_analysis_to_value(&analysis))
        }
        3 => {
            let max_triples = opt_usize(&parsed, "max-triples", 20_000)?;
            let analysis =
                crate::analyze::analyze_triples(&net, &oracle, limit, max_triples).map_err(err)?;
            emit(&wire::triple_analysis_to_value(&analysis))
        }
        other => Err(format!("--depth 只支持 1/2/3，得到 {other}")),
    }
}

pub(super) fn train_proposer(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(
        &parsed,
        "train-proposer <artifact|-> --oracle <json|-> --out <model.json>",
    )?;
    let oracle_path = value(&parsed, "oracle")?;
    let out = value(&parsed, "out")?;
    no_double_stdin(&[path, oracle_path])?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let analysis = crate::analyze::analyze(&net, &oracle, 0).map_err(err)?;
    if analysis.before_loss == 0 {
        return Err("工件已经精确，没有可学习的收益".into());
    }
    let samples = learned::samples_from_analysis(&net, &oracle, &analysis).map_err(err)?;
    let epochs = opt_usize(&parsed, "epochs", 200)?;
    let lr = parsed.values.get("lr").map_or(Ok(0.05), |raw| {
        raw.parse::<f32>()
            .map_err(|_| format!("--lr 需要数字，得到 `{raw}`"))
    })?;
    let holdout = parsed.values.get("holdout").map_or(Ok(0.2), |raw| {
        raw.parse::<f32>()
            .map_err(|_| format!("--holdout 需要数字，得到 `{raw}`"))
    })?;
    let seed = opt_u64(&parsed, "seed", 7)?;
    let (scorer, report) = learned::train_holdout(&samples, holdout, epochs, lr, seed);
    let text = serde_json::to_string_pretty(&scorer.to_json())
        .map_err(|error| format!("模型 JSON 序列化失败：{error}"))?;
    atomic_write(out, &text)?;
    let baseline = if report.holdout > 0 {
        report.holdout_positive as f64 / report.holdout as f64
    } else {
        0.0
    };
    let hit_rate = if report.top_k > 0 {
        report.top_k_hit as f64 / report.top_k as f64
    } else {
        0.0
    };
    emit(&json!({
        "out": out,
        "model": {
            "features": learned::FEATURE_NAMES.len(),
            "samples": scorer.samples,
            "epochs": scorer.epochs,
            "lr": scorer.lr,
        },
        "report": {
            "samples": report.samples,
            "train": report.train,
            "holdout": report.holdout,
            "epochs": report.epochs,
            "lr": report.lr,
            "train_mse": report.train_mse,
            "holdout_mse": report.holdout_mse,
            "holdout_positive": report.holdout_positive,
            "top_k": report.top_k,
            "top_k_hit": report.top_k_hit,
            "top_k_hit_rate": hit_rate,
            "baseline": baseline,
        },
    }))
}

pub(super) fn propose_learned(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &["verify"])?;
    let path = one(
        &parsed,
        "propose-learned <artifact|-> --oracle <json|-> --model <model.json>",
    )?;
    let oracle_path = value(&parsed, "oracle")?;
    let model_path = value(&parsed, "model")?;
    let limit = opt_usize(&parsed, "limit", 16)?;
    no_double_stdin(&[path, oracle_path])?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let text = fs::read_to_string(model_path).map_err(|error| format!("{model_path}: {error}"))?;
    let value: Value =
        serde_json::from_str(&text).map_err(|error| format!("{model_path} JSON 非法：{error}"))?;
    let scorer = Scorer::from_json(&value).map_err(err)?;
    let evaluated = crate::repair::candidates(&net).len();
    let scored = learned::propose(&net, &oracle, &scorer, limit).map_err(err)?;
    let before_loss = oracle.loss(&net).map_err(err)?;
    let verify = parsed.switches.contains("verify");
    let mut candidates = Vec::with_capacity(scored.len());
    for scored in &scored {
        let mut item = json!({
            "edit": wire::edit_to_value(&scored.edit),
            "score": scored.score,
        });
        if verify {
            let mut trial = net.clone();
            scored.edit.apply_to(&mut trial).map_err(err)?;
            let after_loss = oracle.loss(&trial).map_err(err)?;
            item["after_loss"] = json!(after_loss);
            item["gain"] = json!(before_loss as isize - after_loss as isize);
        }
        candidates.push(item);
    }
    emit(&json!({
        "before_loss": before_loss,
        "evaluated": evaluated,
        "limit": limit,
        "verified": verify,
        "model": {
            "samples": scorer.samples,
            "epochs": scorer.epochs,
            "lr": scorer.lr,
        },
        "candidates": candidates,
    }))
}
