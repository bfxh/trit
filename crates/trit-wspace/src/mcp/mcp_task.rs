//! MCP `wsp_task_merge`：任务向量合并（从 `mcp.rs` 拆出，控制单文件行数）。

use serde_json::{Value, json};

use crate::{Patch, to_wsp, wire};

use super::{err, flag, load_key, opt_u64, opt_usize, optional_oracle, parse_or_path};

pub(super) fn task_merge(args: &Value) -> Result<Value, String> {
    let base = load_key(args, "base")?;
    let raws = args
        .get("tasks")
        .and_then(Value::as_array)
        .ok_or("需要 tasks 数组")?;
    let mut tasks = Vec::with_capacity(raws.len());
    let aligned = flag(args, "aligned");
    for raw in raws {
        let text = raw.as_str().ok_or("tasks 必须是字符串数组")?;
        let tuned = parse_or_path(text)?;
        let vector = if aligned {
            crate::task::TaskVector::between_aligned(&base, &tuned)
        } else {
            crate::task::TaskVector::between(&base, &tuned)
        };
        tasks.push(vector.map_err(err)?);
    }
    let dare_rate = args
        .get("dare_rate")
        .and_then(Value::as_f64)
        .map(|rate| rate as f32);
    let dare_per_task = args
        .get("dare_per_task")
        .and_then(Value::as_f64)
        .map(|rate| rate as f32);
    let dare_seed = opt_u64(args, "dare_seed", 0);
    let oracle = optional_oracle(args)?;
    let tune_weights = flag(args, "tune_weights");
    let mut weights = match args.get("weights") {
        None | Some(Value::Null) => None,
        Some(Value::Array(items)) => {
            let mut parsed = Vec::with_capacity(items.len());
            for item in items {
                let value = item.as_f64().ok_or("weights 必须全是数字")?;
                parsed.push(value as f32);
            }
            Some(parsed)
        }
        Some(_) => return Err("weights 必须是数字数组".into()),
    };
    let mut tuning = None;
    if tune_weights {
        let Some(oracle_ref) = oracle.as_ref() else {
            return Err("tune_weights 需要 oracle".into());
        };
        if weights.is_some() {
            return Err("tune_weights 与 weights 只能给一个".into());
        }
        let rounds = opt_usize(args, "tune_rounds", 1);
        let metric_name = args
            .get("tune_metric")
            .and_then(Value::as_str)
            .unwrap_or("exact");
        let metric = match metric_name {
            "exact" => crate::tune::TuneMetric::Exact,
            "hamming" => crate::tune::TuneMetric::Hamming,
            "argmax" => crate::tune::TuneMetric::Argmax,
            "readout" => crate::tune::TuneMetric::Readout,
            other => {
                return Err(format!(
                    "tune_metric 只支持 exact / hamming / argmax / readout，得到 `{other}`"
                ));
            }
        };
        let grid = match args.get("tune_grid") {
            None | Some(Value::Null) => crate::tune::DEFAULT_TUNE_GRID.to_vec(),
            Some(Value::Array(items)) => {
                let mut parsed = Vec::with_capacity(items.len());
                for item in items {
                    let value = item.as_f64().ok_or("tune_grid 必须全是数字")?;
                    parsed.push(value as f32);
                }
                parsed
            }
            Some(_) => return Err("tune_grid 必须是数字数组".into()),
        };
        let merge = if flag(args, "tune_with_dare") {
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
                    return Err("tune_with_dare 需要 dare_rate 或 dare_per_task".into());
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
            &base, &tasks, oracle_ref, rounds, metric, &grid, merge,
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
                return Err("dare_rate 与 dare_per_task 只能给一个".into());
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
    let verify = if let Some(oracle) = oracle.as_ref() {
        Some(json!({
            "before": wire::metrics_value(&base, oracle).map_err(err)?,
            "after": wire::metrics_value(&merged, oracle).map_err(err)?,
        }))
    } else {
        None
    };
    let patch = Patch::diff(&base, &merged).map_err(err)?;
    Ok(json!({
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
