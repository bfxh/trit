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
        let metric = args
            .get("tune_metric")
            .and_then(Value::as_str)
            .unwrap_or("exact");
        let report = match metric {
            "exact" => crate::task::tune_task_weights(&base, &tasks, oracle_ref, rounds),
            "hamming" => crate::task::tune_task_weights_hamming(&base, &tasks, oracle_ref, rounds),
            "argmax" => crate::task::tune_task_weights_argmax(&base, &tasks, oracle_ref, rounds),
            "readout" => crate::task::tune_task_weights_readout(&base, &tasks, oracle_ref, rounds),
            other => {
                return Err(format!(
                    "tune_metric 只支持 exact / hamming / argmax / readout，得到 `{other}`"
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
        (Some(_), Some(_), _) => {
            return Err("dare_rate 与 dare_per_task 只能给一个".into());
        }
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
