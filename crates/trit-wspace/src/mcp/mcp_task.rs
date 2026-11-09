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
            "before_classification": wire::classification_value(&base, oracle).map_err(err)?,
            "after_classification": wire::classification_value(&merged, oracle).map_err(err)?,
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

pub(super) fn task_scale(args: &Value) -> Result<Value, String> {
    let base = load_key(args, "base")?;
    let tuned = parse_or_path(
        args.get("tuned")
            .and_then(Value::as_str)
            .ok_or("需要 tuned")?,
    )?;
    let oracle = optional_oracle(args)?;
    let lambda = args.get("lambda").and_then(Value::as_f64).map(|v| v as f32);
    let lambdas = match args.get("lambdas") {
        Some(Value::Array(items)) => {
            let mut values = Vec::new();
            for item in items {
                values.push(item.as_f64().ok_or("lambdas 需要数字数组")? as f32);
            }
            Some(values)
        }
        Some(_) => return Err("lambdas 需要数字数组".into()),
        None => None,
    };
    let layer_lambdas = match args.get("layer_lambdas") {
        Some(Value::Array(items)) => {
            let mut values = Vec::new();
            for item in items {
                values.push(item.as_f64().ok_or("layer_lambdas 需要数字数组")? as f32);
            }
            Some(values)
        }
        Some(_) => return Err("layer_lambdas 需要数字数组".into()),
        None => None,
    };
    let row_lambdas = match args.get("row_lambdas") {
        Some(Value::Array(items)) => {
            let mut values = Vec::new();
            for item in items {
                values.push(item.as_f64().ok_or("row_lambdas 需要数字数组")? as f32);
            }
            Some(values)
        }
        Some(_) => return Err("row_lambdas 需要数字数组".into()),
        None => None,
    };
    let block_lambdas = match args.get("block_lambdas") {
        Some(Value::Array(items)) => {
            let mut values = Vec::new();
            for item in items {
                values.push(item.as_f64().ok_or("block_lambdas 需要数字数组")? as f32);
            }
            Some(values)
        }
        Some(_) => return Err("block_lambdas 需要数字数组".into()),
        None => None,
    };
    if usize::from(lambda.is_some())
        + usize::from(lambdas.is_some())
        + usize::from(layer_lambdas.is_some())
        + usize::from(row_lambdas.is_some())
        + usize::from(block_lambdas.is_some())
        > 1
    {
        return Err("lambda/lambdas/layer_lambdas/row_lambdas/block_lambdas 只能给一个".into());
    }
    let dims: Vec<(usize, usize)> = base
        .layers()
        .iter()
        .map(|layer| (layer.rows(), layer.cols()))
        .collect();
    if let Some(layer_lambdas) = layer_lambdas {
        if layer_lambdas.len() != base.layers().len() {
            return Err(format!(
                "layer_lambdas 需要 {} 个（每层一个），得到 {}",
                base.layers().len(),
                layer_lambdas.len()
            ));
        }
        let vector = crate::task::TaskVector::between(&base, &tuned).map_err(err)?;
        let scaled = crate::scale::scale_task_layers(&vector, &layer_lambdas).map_err(err)?;
        let net = scaled.apply(&base).map_err(err)?;
        let patch = Patch::diff(&base, &net).map_err(err)?;
        let mut row = json!({
            "layer_lambdas": layer_lambdas,
            "summary": wire::net_summary(&net),
            "patch": wire::patch_to_value(&patch),
            "cost": wire::cost_to_value(&patch.cost(&base)),
            "artifact": to_wsp(&net),
        });
        if let Some(oracle) = &oracle {
            row["verify"] = wire::metrics_value(&net, oracle).map_err(err)?;
        }
        return Ok(row);
    }
    let row_map = args
        .get("row_lambdas_map")
        .and_then(Value::as_object)
        .map(parse_row_map)
        .transpose()?;
    let block_map = args
        .get("block_lambdas_map")
        .and_then(Value::as_object)
        .map(parse_block_map)
        .transpose()?;
    if let Some(row_map) = row_map {
        let vector = crate::task::TaskVector::between(&base, &tuned).map_err(err)?;
        let scaled = crate::scale::scale_task_rows_map(&vector, &row_map).map_err(err)?;
        let net = scaled.apply(&base).map_err(err)?;
        let patch = Patch::diff(&base, &net).map_err(err)?;
        let map_value: serde_json::Map<String, Value> = row_map
            .iter()
            .map(|((layer, row), value)| (format!("{layer}:{row}"), json!(value)))
            .collect();
        let mut row = json!({
            "row_lambdas_map": map_value,
            "summary": wire::net_summary(&net),
            "patch": wire::patch_to_value(&patch),
            "cost": wire::cost_to_value(&patch.cost(&base)),
            "artifact": to_wsp(&net),
        });
        if let Some(oracle) = &oracle {
            row["verify"] = wire::metrics_value(&net, oracle).map_err(err)?;
        }
        return Ok(row);
    }
    if let Some(block_map) = block_map {
        let block_size = args
            .get("block_size")
            .and_then(Value::as_u64)
            .ok_or("block_size 需要正整数")? as usize;
        let vector = crate::task::TaskVector::between(&base, &tuned).map_err(err)?;
        let scaled =
            crate::scale::scale_task_blocks_map(&vector, &block_map, block_size).map_err(err)?;
        let net = scaled.apply(&base).map_err(err)?;
        let patch = Patch::diff(&base, &net).map_err(err)?;
        let map_value: serde_json::Map<String, Value> = block_map
            .iter()
            .map(|((layer, row, unit), value)| (format!("{layer}:{row}:{unit}"), json!(value)))
            .collect();
        let mut row = json!({
            "block_lambdas_map": map_value,
            "block_size": block_size,
            "summary": wire::net_summary(&net),
            "patch": wire::patch_to_value(&patch),
            "cost": wire::cost_to_value(&patch.cost(&base)),
            "artifact": to_wsp(&net),
        });
        if let Some(oracle) = &oracle {
            row["verify"] = wire::metrics_value(&net, oracle).map_err(err)?;
        }
        return Ok(row);
    }
    if let Some(row_lambdas) = row_lambdas {
        let total: usize = dims.iter().map(|(rows, _)| *rows).sum();
        if row_lambdas.len() != total {
            return Err(format!(
                "row_lambdas 需要 {total} 个（每行一个），得到 {}",
                row_lambdas.len()
            ));
        }
        let vector = crate::task::TaskVector::between(&base, &tuned).map_err(err)?;
        let scaled = crate::scale::scale_task_rows(&vector, &row_lambdas, &dims).map_err(err)?;
        let net = scaled.apply(&base).map_err(err)?;
        let patch = Patch::diff(&base, &net).map_err(err)?;
        let mut row = json!({
            "row_lambdas": row_lambdas,
            "summary": wire::net_summary(&net),
            "patch": wire::patch_to_value(&patch),
            "cost": wire::cost_to_value(&patch.cost(&base)),
            "artifact": to_wsp(&net),
        });
        if let Some(oracle) = &oracle {
            row["verify"] = wire::metrics_value(&net, oracle).map_err(err)?;
        }
        return Ok(row);
    }
    if let Some(block_lambdas) = block_lambdas {
        let block_size = args
            .get("block_size")
            .and_then(Value::as_u64)
            .ok_or("block_size 需要正整数")? as usize;
        let total: usize = dims
            .iter()
            .map(|(rows, cols)| *rows * cols.div_ceil(block_size))
            .sum();
        if block_lambdas.len() != total {
            return Err(format!(
                "block_lambdas 需要 {total} 个（每块一个），得到 {}",
                block_lambdas.len()
            ));
        }
        let vector = crate::task::TaskVector::between(&base, &tuned).map_err(err)?;
        let scaled = crate::scale::scale_task_blocks(&vector, &block_lambdas, &dims, block_size)
            .map_err(err)?;
        let net = scaled.apply(&base).map_err(err)?;
        let patch = Patch::diff(&base, &net).map_err(err)?;
        let mut row = json!({
            "block_lambdas": block_lambdas,
            "block_size": block_size,
            "summary": wire::net_summary(&net),
            "patch": wire::patch_to_value(&patch),
            "cost": wire::cost_to_value(&patch.cost(&base)),
            "artifact": to_wsp(&net),
        });
        if let Some(oracle) = &oracle {
            row["verify"] = wire::metrics_value(&net, oracle).map_err(err)?;
        }
        return Ok(row);
    }
    let values = match lambdas {
        Some(values) => values,
        None => vec![lambda.ok_or("需要 lambda 或 lambdas")?],
    };
    let vector = crate::task::TaskVector::between(&base, &tuned).map_err(err)?;
    let mut results = Vec::new();
    let mut single: Option<crate::Net> = None;
    for lambda in values {
        let scaled = crate::scale::scale_task(&vector, lambda).map_err(err)?;
        let net = scaled.apply(&base).map_err(err)?;
        let patch = Patch::diff(&base, &net).map_err(err)?;
        let mut row = json!({
            "lambda": lambda,
            "summary": wire::net_summary(&net),
            "patch": wire::patch_to_value(&patch),
            "cost": wire::cost_to_value(&patch.cost(&base)),
        });
        if let Some(oracle) = &oracle {
            row["verify"] = wire::metrics_value(&net, oracle).map_err(err)?;
        }
        results.push(row);
        single = Some(net);
    }
    if results.len() == 1 {
        let net = single.expect("single result");
        let mut out = results.pop().expect("single row");
        out["artifact"] = json!(to_wsp(&net));
        Ok(out)
    } else {
        let lambdas: Vec<serde_json::Value> =
            results.iter().map(|row| row["lambda"].clone()).collect();
        Ok(json!({"lambdas": lambdas, "results": results}))
    }
}

pub(super) fn task_combine(args: &Value) -> Result<Value, String> {
    let base = load_key(args, "base")?;
    let task_values = args
        .get("tasks")
        .and_then(Value::as_array)
        .ok_or("需要 tasks 数组")?;
    let lambdas: Vec<f32> = args
        .get("lambdas")
        .and_then(Value::as_array)
        .ok_or("需要 lambdas 数组")?
        .iter()
        .map(|item| {
            item.as_f64()
                .map(|value| value as f32)
                .ok_or_else(|| "lambdas 需要数字数组".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut tasks = Vec::with_capacity(task_values.len());
    for item in task_values {
        let text = item.as_str().ok_or("tasks 需要字符串数组")?;
        let net = parse_or_path(text)?;
        tasks.push(crate::task::TaskVector::between(&base, &net).map_err(err)?);
    }
    let merged = crate::scale::combine_tasks(&base, &tasks, &lambdas).map_err(err)?;
    let patch = Patch::diff(&base, &merged).map_err(err)?;
    Ok(json!({
        "lambdas": lambdas,
        "summary": wire::net_summary(&merged),
        "patch": wire::patch_to_value(&patch),
        "cost": wire::cost_to_value(&patch.cost(&base)),
        "artifact": to_wsp(&merged),
    }))
}

fn parse_row_map(
    object: &serde_json::Map<String, Value>,
) -> Result<std::collections::BTreeMap<(usize, usize), f32>, String> {
    let mut map = std::collections::BTreeMap::new();
    for (key, value) in object {
        let parts: Vec<&str> = key.split(':').collect();
        if parts.len() != 2 {
            return Err(format!("键 {key} 需要 layer:row"));
        }
        let layer = parts[0]
            .parse::<usize>()
            .map_err(|_| format!("键 {key} 的 layer 非法"))?;
        let row = parts[1]
            .parse::<usize>()
            .map_err(|_| format!("键 {key} 的 row 非法"))?;
        let lambda = value.as_f64().ok_or_else(|| format!("键 {key} 需要数字"))? as f32;
        map.insert((layer, row), lambda);
    }
    Ok(map)
}

fn parse_block_map(
    object: &serde_json::Map<String, Value>,
) -> Result<std::collections::BTreeMap<(usize, usize, usize), f32>, String> {
    let mut map = std::collections::BTreeMap::new();
    for (key, value) in object {
        let parts: Vec<&str> = key.split(':').collect();
        if parts.len() != 3 {
            return Err(format!("键 {key} 需要 layer:row:block"));
        }
        let layer = parts[0]
            .parse::<usize>()
            .map_err(|_| format!("键 {key} 的 layer 非法"))?;
        let row = parts[1]
            .parse::<usize>()
            .map_err(|_| format!("键 {key} 的 row 非法"))?;
        let unit = parts[2]
            .parse::<usize>()
            .map_err(|_| format!("键 {key} 的 block 非法"))?;
        let lambda = value.as_f64().ok_or_else(|| format!("键 {key} 需要数字"))? as f32;
        map.insert((layer, row, unit), lambda);
    }
    Ok(map)
}
