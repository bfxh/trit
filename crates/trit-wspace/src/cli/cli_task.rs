//! CLI `task-merge`：加权任务向量合并（从 `cli.rs` 拆出，控制单文件行数）。

use serde_json::json;

use crate::edit::Patch;
use crate::to_wsp;
use crate::wire;

use super::{
    emit, err, load_net, load_oracle, no_double_stdin, opt_u64, opt_usize, parse, req_f32,
    write_out,
};

pub(super) fn task_merge(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &["aligned", "tune-weights", "tune-with-dare"])?;
    if parsed.positional.len() < 2 {
        return Err(
            "用法：trit-wspace task-merge <base|-> <tuned|->... [--aligned] [--weight W[,W...]] [--dare-rate R|--dare-per-task R] [--dare-seed N] [--oracle <json|->] [--tune-weights [--tune-rounds N] [--tune-grid G[,G...]] [--tune-with-dare]] [--out <path>]".into(),
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
        let metric_name = parsed
            .values
            .get("tune-metric")
            .map_or("exact", String::as_str);
        let metric = match metric_name {
            "exact" => crate::tune::TuneMetric::Exact,
            "hamming" => crate::tune::TuneMetric::Hamming,
            "argmax" => crate::tune::TuneMetric::Argmax,
            "readout" => crate::tune::TuneMetric::Readout,
            other => {
                return Err(format!(
                    "--tune-metric 只支持 exact / hamming / argmax / readout，得到 `{other}`"
                ));
            }
        };
        let grid = parsed
            .values
            .get("tune-grid")
            .map(|raw| parse_weights(raw))
            .transpose()?
            .unwrap_or_else(|| crate::tune::DEFAULT_TUNE_GRID.to_vec());
        let merge = if parsed.switches.contains("tune-with-dare") {
            match (dare_rate, dare_per_task) {
                (Some(rate), None) => crate::tune::TuneMerge::Dare {
                    rate,
                    seed: dare_seed,
                },
                (None, Some(rate)) => crate::tune::TuneMerge::PerTaskDare {
                    rate,
                    seed: dare_seed,
                },
                _ => {
                    return Err("--tune-with-dare 需要 --dare-rate 或 --dare-per-task".into());
                }
            }
        } else {
            crate::tune::TuneMerge::Plain
        };
        let merge_label = match merge {
            crate::tune::TuneMerge::Plain => "plain",
            crate::tune::TuneMerge::Dare { .. } => "dare",
            crate::tune::TuneMerge::PerTaskDare { .. } => "per-task-dare",
        };
        let report = crate::tune::tune_task_weights_metric(
            &base,
            &tasks,
            &tuning_oracle,
            rounds,
            metric,
            &grid,
            merge,
        )
        .map_err(err)?;
        weights = Some(report.weights().to_vec());
        tuning = Some(json!({
            "rounds": rounds,
            "metric": metric_name,
            "grid": grid,
            "merge": merge_label,
            "evaluations": report.evaluations(),
            "initial_loss": report.initial_loss(),
            "tuned_loss": report.loss(),
        }));
    }
    let ones = vec![1.0_f32; tasks.len()];
    let effective_weights = weights.as_deref().unwrap_or(&ones);
    let (merged, merge_report) = if dare_rate.is_none() && dare_per_task.is_none() {
        let (net, report) =
            crate::task::merge_tasks_weighted_report(&base, &tasks, effective_weights)
                .map_err(err)?;
        (net, Some(report))
    } else {
        let net = match (dare_rate, dare_per_task) {
            (Some(_), Some(_)) => {
                return Err("--dare-rate 与 --dare-per-task 只能给一个".into());
            }
            (Some(rate), None) => match weights.as_deref() {
                Some(weights) => {
                    crate::task::merge_tasks_weighted_dare(&base, &tasks, weights, rate, dare_seed)
                        .map_err(err)?
                }
                None => {
                    crate::task::merge_tasks_dare(&base, &tasks, rate, dare_seed).map_err(err)?
                }
            },
            (None, Some(rate)) => match weights.as_deref() {
                Some(weights) => crate::task::merge_tasks_weighted_per_task_dare(
                    &base, &tasks, weights, rate, dare_seed,
                )
                .map_err(err)?,
                None => crate::task::merge_tasks_per_task_dare(&base, &tasks, rate, dare_seed)
                    .map_err(err)?,
            },
            (None, None) => unreachable!(),
        };
        (net, None)
    };
    write_out(&parsed, &merged)?;
    let verify = if let Some(path) = oracle_path {
        let oracle = load_oracle(path)?;
        Some(json!({
            "before": wire::metrics_value(&base, &oracle).map_err(err)?,
            "after": wire::metrics_value(&merged, &oracle).map_err(err)?,
            "before_classification": wire::classification_value(&base, &oracle).map_err(err)?,
            "after_classification": wire::classification_value(&merged, &oracle).map_err(err)?,
        }))
    } else {
        None
    };
    let patch = Patch::diff(&base, &merged).map_err(err)?;
    emit(&json!({
        "tasks": tasks.len(),
        "weights": weights,
        "tuning": tuning,
        "merge_report": merge_report.map(|report| json!({
            "tasks": report.tasks(),
            "ignored_tasks": report.ignored_tasks(),
            "voted": report.voted(),
            "ties": report.ties(),
            "changed": report.changed(),
            "scale_rows": report.scale_rows(),
        })),
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

pub(super) fn task_scale(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    if parsed.positional.len() != 2 {
        return Err(
            "用法：trit-wspace task-scale <base|-> <tuned|-> --lambda L [--out <path>]".into(),
        );
    }
    no_double_stdin(&parsed.positional)?;
    let base = load_net(&parsed.positional[0])?;
    let tuned = load_net(&parsed.positional[1])?;
    let lambda = req_f32(&parsed, "lambda")?;
    let vector = crate::task::TaskVector::between(&base, &tuned).map_err(err)?;
    let scaled = crate::task::scale_task(&vector, lambda).map_err(err)?;
    let net = scaled.apply(&base).map_err(err)?;
    write_out(&parsed, &net)?;
    let patch = Patch::diff(&base, &net).map_err(err)?;
    emit(&json!({
        "lambda": lambda,
        "summary": wire::net_summary(&net),
        "patch": wire::patch_to_value(&patch),
        "cost": wire::cost_to_value(&patch.cost(&base)),
        "artifact": to_wsp(&net),
    }))
}
