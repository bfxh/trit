//! CLI `task-merge`：加权任务向量合并（从 `cli.rs` 拆出，控制单文件行数）。

use serde_json::json;

use crate::edit::Patch;
use crate::to_wsp;
use crate::wire;

use super::{
    emit, err, load_net, load_oracle, no_double_stdin, opt_u64, opt_usize, parse, write_out,
};

pub(super) fn task_merge(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &["aligned", "tune-weights"])?;
    if parsed.positional.len() < 2 {
        return Err(
            "用法：trit-wspace task-merge <base|-> <tuned|->... [--aligned] [--weight W[,W...]] [--dare-rate R|--dare-per-task R] [--dare-seed N] [--oracle <json|->] [--tune-weights [--tune-rounds N]] [--out <path>]".into(),
        );
    }
    let oracle_path = parsed.values.get("oracle");
    let mut sources: Vec<&str> = parsed.positional.iter().map(String::as_str).collect();
    if let Some(path) = oracle_path {
        sources.push(path.as_str());
    }
    no_double_stdin(&sources)?;
    let aligned = parsed.switches.contains("aligned");
    let base = load_net(&parsed.positional[0])?;
    let mut tasks = Vec::with_capacity(parsed.positional.len() - 1);
    for path in &parsed.positional[1..] {
        let tuned = load_net(path)?;
        let vector = if aligned {
            crate::task::TaskVector::between_aligned(&base, &tuned)
        } else {
            crate::task::TaskVector::between(&base, &tuned)
        };
        tasks.push(vector.map_err(err)?);
    }
    let dare_rate = parsed
        .values
        .get("dare-rate")
        .map(|raw| raw.parse::<f32>())
        .transpose()
        .map_err(|_| "--dare-rate 需要数字".to_string())?;
    let dare_per_task = parsed
        .values
        .get("dare-per-task")
        .map(|raw| raw.parse::<f32>())
        .transpose()
        .map_err(|_| "--dare-per-task 需要数字".to_string())?;
    let dare_seed = opt_u64(&parsed, "dare-seed", 0)?;
    let mut weights = parsed
        .values
        .get("weight")
        .map(|raw| parse_weights(raw))
        .transpose()?;
    let mut tuning = None;
    if parsed.switches.contains("tune-weights") {
        let Some(path) = oracle_path else {
            return Err("--tune-weights 需要 --oracle".into());
        };
        if weights.is_some() {
            return Err("--tune-weights 与 --weight 只能给一个".into());
        }
        let tuning_oracle = load_oracle(path)?;
        let rounds = opt_usize(&parsed, "tune-rounds", 1)?;
        let metric = parsed
            .values
            .get("tune-metric")
            .map_or("exact", String::as_str);
        let report = match metric {
            "exact" => crate::task::tune_task_weights(&base, &tasks, &tuning_oracle, rounds),
            "hamming" => {
                crate::task::tune_task_weights_hamming(&base, &tasks, &tuning_oracle, rounds)
            }
            "argmax" => {
                crate::task::tune_task_weights_argmax(&base, &tasks, &tuning_oracle, rounds)
            }
            "readout" => {
                crate::task::tune_task_weights_readout(&base, &tasks, &tuning_oracle, rounds)
            }
            other => {
                return Err(format!(
                    "--tune-metric 只支持 exact / hamming / argmax / readout，得到 `{other}`"
                ));
            }
        }
        .map_err(err)?;
        weights = Some(report.weights().to_vec());
        tuning = Some(json!({
            "rounds": rounds,
            "metric": metric,
            "evaluations": report.evaluations(),
            "initial_loss": report.initial_loss(),
            "tuned_loss": report.loss(),
        }));
    }
    let merged = match (dare_rate, dare_per_task, weights.as_deref()) {
        (Some(_), Some(_), _) => return Err("--dare-rate 与 --dare-per-task 只能给一个".into()),
        (Some(rate), None, Some(weights)) => {
            crate::task::merge_tasks_weighted_dare(&base, &tasks, weights, rate, dare_seed)
                .map_err(err)?
        }
        (Some(rate), None, None) => {
            crate::task::merge_tasks_dare(&base, &tasks, rate, dare_seed).map_err(err)?
        }
        (None, Some(rate), Some(weights)) => {
            crate::task::merge_tasks_weighted_per_task_dare(&base, &tasks, weights, rate, dare_seed)
                .map_err(err)?
        }
        (None, Some(rate), None) => {
            crate::task::merge_tasks_per_task_dare(&base, &tasks, rate, dare_seed).map_err(err)?
        }
        (None, None, Some(weights)) => {
            crate::task::merge_tasks_weighted(&base, &tasks, weights).map_err(err)?
        }
        (None, None, None) => crate::task::merge_tasks(&base, &tasks).map_err(err)?,
    };
    write_out(&parsed, &merged)?;
    let verify = if let Some(path) = oracle_path {
        let oracle = load_oracle(path)?;
        Some(json!({
            "before": wire::metrics_value(&base, &oracle).map_err(err)?,
            "after": wire::metrics_value(&merged, &oracle).map_err(err)?,
        }))
    } else {
        None
    };
    let patch = Patch::diff(&base, &merged).map_err(err)?;
    emit(&json!({
        "tasks": tasks.len(),
        "weights": weights,
        "tuning": tuning,
        "verify": verify,
        "dare_rate": dare_rate,
        "dare_per_task": dare_per_task,
        "dare_seed": dare_seed,
        "summary": wire::net_summary(&merged),
        "patch": wire::patch_to_value(&patch),
        "cost": wire::cost_to_value(&patch.cost(&base)),
        "artifact": to_wsp(&merged),
    }))
}

fn parse_weights(raw: &str) -> Result<Vec<f32>, String> {
    if raw.is_empty() {
        return Err("--weight 不能为空".into());
    }
    raw.split(',')
        .map(|part| {
            part.trim()
                .parse::<f32>()
                .map_err(|_| format!("--weight 需要逗号分隔的数字，得到 `{part}`"))
        })
        .collect()
}
