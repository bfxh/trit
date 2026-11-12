//! CLI `task-merge`：加权任务向量合并（从 `cli.rs` 拆出，控制单文件行数）。

use serde_json::{Value, json};

use crate::edit::Patch;
use crate::{task_merge, to_wsp, wire};

use super::{
    emit, err, load_net, load_oracle, no_double_stdin, one, opt_u64, opt_usize, parse, req_f32,
    req_usize, value, write_out,
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
            task_merge::merge_tasks_weighted_report(&base, &tasks, effective_weights)
                .map_err(err)?;
        (net, Some(report))
    } else {
        let (net, report) = match (dare_rate, dare_per_task) {
            (Some(_), Some(_)) => return Err("--dare-rate 与 --dare-per-task 只能给一个".into()),
            (Some(rate), None) => match weights.as_deref() {
                Some(weights) => task_merge::merge_tasks_weighted_dare_report(
                    &base, &tasks, weights, rate, dare_seed,
                )
                .map_err(err)?,
                None => task_merge::merge_tasks_dare_report(&base, &tasks, rate, dare_seed)
                    .map_err(err)?,
            },
            (None, Some(rate)) => match weights.as_deref() {
                Some(weights) => task_merge::merge_tasks_weighted_per_task_dare_report(
                    &base, &tasks, weights, rate, dare_seed,
                )
                .map_err(err)?,
                None => {
                    task_merge::merge_tasks_per_task_dare_report(&base, &tasks, rate, dare_seed)
                        .map_err(err)?
                }
            },
            (None, None) => unreachable!(),
        };
        (net, Some(report))
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
        "merge_report": merge_report.map(|report| report.to_json()),
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
            "用法：trit-wspace task-scale <base|-> <tuned|-> (--lambda L | --lambdas L,L... | --layer-lambdas L0,L1... | --row-lambdas L... | --block-lambdas L... --block-size N) [--oracle <json|->] [--out <path>]"
                .into(),
        );
    }
    let mut sources = vec![parsed.positional[0].as_str(), parsed.positional[1].as_str()];
    if let Some(oracle) = parsed.values.get("oracle") {
        sources.push(oracle.as_str());
    }
    no_double_stdin(&sources)?;
    let base = load_net(&parsed.positional[0])?;
    let tuned = load_net(&parsed.positional[1])?;
    let oracle = parsed
        .values
        .get("oracle")
        .map(|path| load_oracle(path))
        .transpose()?;
    let lambda = if parsed.values.contains_key("lambda") {
        Some(req_f32(&parsed, "lambda")?)
    } else {
        None
    };
    let lambdas = parsed
        .values
        .get("lambdas")
        .map(|raw| parse_lambdas(raw))
        .transpose()?;
    let layer_lambdas = parsed
        .values
        .get("layer-lambdas")
        .map(|raw| parse_lambdas(raw))
        .transpose()?;
    let row_lambdas = parsed
        .values
        .get("row-lambdas")
        .map(|raw| parse_lambdas(raw))
        .transpose()?;
    let block_lambdas = parsed
        .values
        .get("block-lambdas")
        .map(|raw| parse_lambdas(raw))
        .transpose()?;
    let row_map = parsed
        .values
        .get("row-lambdas-file")
        .map(|path| read_json(path).and_then(|raw| parse_row_map(&raw)))
        .transpose()?;
    let block_map = parsed
        .values
        .get("block-lambdas-file")
        .map(|path| read_json(path).and_then(|raw| parse_block_map(&raw)))
        .transpose()?;
    let row_patterns = parsed
        .values
        .get("row-lambdas-patterns")
        .map(|path| {
            read_json(path).and_then(|raw| parse_unit_patterns(&raw, 2, "--row-lambdas-patterns"))
        })
        .transpose()?;
    let block_patterns = parsed
        .values
        .get("block-lambdas-patterns")
        .map(|path| {
            read_json(path).and_then(|raw| parse_unit_patterns(&raw, 3, "--block-lambdas-patterns"))
        })
        .transpose()?;
    if usize::from(lambda.is_some())
        + usize::from(lambdas.is_some())
        + usize::from(layer_lambdas.is_some())
        + usize::from(row_lambdas.is_some())
        + usize::from(block_lambdas.is_some())
        + usize::from(row_map.is_some())
        + usize::from(block_map.is_some())
        + usize::from(row_patterns.is_some())
        + usize::from(block_patterns.is_some())
        > 1
    {
        return Err(
            "--lambda/--lambdas/--layer-lambdas/--row-lambdas/--block-lambdas 只能给一个".into(),
        );
    }
    let dims: Vec<(usize, usize)> = base
        .layers()
        .iter()
        .map(|layer| (layer.rows(), layer.cols()))
        .collect();
    if let Some(layer_lambdas) = layer_lambdas {
        if layer_lambdas.len() != base.layers().len() {
            return Err(format!(
                "--layer-lambdas 需要 {} 个（每层一个），得到 {}",
                base.layers().len(),
                layer_lambdas.len()
            ));
        }
        let vector = crate::task::TaskVector::between(&base, &tuned).map_err(err)?;
        let scaled = crate::scale::scale_task_layers(&vector, &layer_lambdas).map_err(err)?;
        let net = scaled.apply(&base).map_err(err)?;
        write_out(&parsed, &net)?;
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
        return emit(&row);
    }
    if let Some(row_lambdas) = row_lambdas {
        let total: usize = dims.iter().map(|(rows, _)| *rows).sum();
        if row_lambdas.len() != total {
            return Err(format!(
                "--row-lambdas 需要 {total} 个（每行一个），得到 {}",
                row_lambdas.len()
            ));
        }
        let vector = crate::task::TaskVector::between(&base, &tuned).map_err(err)?;
        let scaled = crate::scale::scale_task_rows(&vector, &row_lambdas, &dims).map_err(err)?;
        let net = scaled.apply(&base).map_err(err)?;
        write_out(&parsed, &net)?;
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
        return emit(&row);
    }
    if let Some(row_map) = row_map {
        let vector = crate::task::TaskVector::between(&base, &tuned).map_err(err)?;
        let scaled = crate::scale::scale_task_rows_map(&vector, &row_map).map_err(err)?;
        let net = scaled.apply(&base).map_err(err)?;
        write_out(&parsed, &net)?;
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
        return emit(&row);
    }
    if let Some(block_map) = block_map {
        let block_size = req_usize(&parsed, "block-size")?;
        let vector = crate::task::TaskVector::between(&base, &tuned).map_err(err)?;
        let scaled =
            crate::scale::scale_task_blocks_map(&vector, &block_map, block_size).map_err(err)?;
        let net = scaled.apply(&base).map_err(err)?;
        write_out(&parsed, &net)?;
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
        return emit(&row);
    }
    if let Some(patterns) = row_patterns {
        let vector = crate::task::TaskVector::between(&base, &tuned).map_err(err)?;
        let scaled = crate::scale::scale_task_rows_patterns(&vector, &patterns).map_err(err)?;
        let net = scaled.apply(&base).map_err(err)?;
        write_out(&parsed, &net)?;
        let patch = Patch::diff(&base, &net).map_err(err)?;
        let map_value: serde_json::Map<String, Value> = patterns
            .iter()
            .map(|pattern| (render_pattern(&pattern.parts), json!(pattern.lambda)))
            .collect();
        let mut row = json!({
            "row_lambdas_patterns": map_value,
            "summary": wire::net_summary(&net),
            "patch": wire::patch_to_value(&patch),
            "cost": wire::cost_to_value(&patch.cost(&base)),
            "artifact": to_wsp(&net),
        });
        if let Some(oracle) = &oracle {
            row["verify"] = wire::metrics_value(&net, oracle).map_err(err)?;
        }
        return emit(&row);
    }
    if let Some(patterns) = block_patterns {
        let block_size = req_usize(&parsed, "block-size")?;
        let vector = crate::task::TaskVector::between(&base, &tuned).map_err(err)?;
        let scaled = crate::scale::scale_task_blocks_patterns(&vector, &patterns, block_size)
            .map_err(err)?;
        let net = scaled.apply(&base).map_err(err)?;
        write_out(&parsed, &net)?;
        let patch = Patch::diff(&base, &net).map_err(err)?;
        let map_value: serde_json::Map<String, Value> = patterns
            .iter()
            .map(|pattern| (render_pattern(&pattern.parts), json!(pattern.lambda)))
            .collect();
        let mut row = json!({
            "block_lambdas_patterns": map_value,
            "block_size": block_size,
            "summary": wire::net_summary(&net),
            "patch": wire::patch_to_value(&patch),
            "cost": wire::cost_to_value(&patch.cost(&base)),
            "artifact": to_wsp(&net),
        });
        if let Some(oracle) = &oracle {
            row["verify"] = wire::metrics_value(&net, oracle).map_err(err)?;
        }
        return emit(&row);
    }
    if let Some(block_lambdas) = block_lambdas {
        let block_size = req_usize(&parsed, "block-size")?;
        let total: usize = dims
            .iter()
            .map(|(rows, cols)| *rows * cols.div_ceil(block_size))
            .sum();
        if block_lambdas.len() != total {
            return Err(format!(
                "--block-lambdas 需要 {total} 个（每块一个），得到 {}",
                block_lambdas.len()
            ));
        }
        let vector = crate::task::TaskVector::between(&base, &tuned).map_err(err)?;
        let scaled = crate::scale::scale_task_blocks(&vector, &block_lambdas, &dims, block_size)
            .map_err(err)?;
        let net = scaled.apply(&base).map_err(err)?;
        write_out(&parsed, &net)?;
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
        return emit(&row);
    }
    let values = match lambdas {
        Some(values) => values,
        None => vec![lambda.ok_or("需要 --lambda 或 --lambdas")?],
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
        write_out(&parsed, &net)?;
        let mut out = results.pop().expect("single row");
        out["artifact"] = json!(to_wsp(&net));
        emit(&out)
    } else {
        let lambdas: Vec<serde_json::Value> =
            results.iter().map(|row| row["lambda"].clone()).collect();
        emit(&json!({"lambdas": lambdas, "results": results}))
    }
}

fn parse_lambdas(raw: &str) -> Result<Vec<f32>, String> {
    if raw.is_empty() {
        return Err("--lambdas 不能为空".into());
    }
    raw.split(',')
        .map(|part| {
            part.trim()
                .parse::<f32>()
                .map_err(|_| format!("--lambdas 需要逗号分隔的数字，得到 `{part}`"))
        })
        .collect()
}

pub(super) fn task_combine(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(
        &parsed,
        "task-combine <base|-> --tasks a,b,... --lambdas L,L... [--out <path>]",
    )?;
    let task_paths = parse_paths(value(&parsed, "tasks")?)?;
    let lambdas = parse_lambdas(value(&parsed, "lambdas")?)?;
    let mut sources = vec![path];
    sources.extend(task_paths.iter().map(String::as_str));
    no_double_stdin(&sources)?;
    let base = load_net(path)?;
    let mut tasks = Vec::with_capacity(task_paths.len());
    for task_path in &task_paths {
        let net = load_net(task_path)?;
        tasks.push(crate::task::TaskVector::between(&base, &net).map_err(err)?);
    }
    let merged = crate::scale::combine_tasks(&base, &tasks, &lambdas).map_err(err)?;
    write_out(&parsed, &merged)?;
    let patch = Patch::diff(&base, &merged).map_err(err)?;
    emit(&json!({
        "lambdas": lambdas,
        "summary": wire::net_summary(&merged),
        "patch": wire::patch_to_value(&patch),
        "cost": wire::cost_to_value(&patch.cost(&base)),
        "artifact": to_wsp(&merged),
    }))
}

fn parse_paths(raw: &str) -> Result<Vec<String>, String> {
    let paths: Vec<String> = raw
        .split(',')
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect();
    if paths.is_empty() {
        return Err("--tasks 不能为空".into());
    }
    Ok(paths)
}

fn read_json(path: &str) -> Result<String, String> {
    if path == "-" {
        std::io::read_to_string(std::io::stdin()).map_err(|error| error.to_string())
    } else {
        std::fs::read_to_string(path).map_err(|error| format!("{path}: {error}"))
    }
}

fn parse_row_map(raw: &str) -> Result<std::collections::BTreeMap<(usize, usize), f32>, String> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|error| format!("--row-lambdas-file 需要 JSON 对象: {error}"))?;
    let object = value
        .as_object()
        .ok_or("--row-lambdas-file 需要 JSON 对象")?;
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
    raw: &str,
) -> Result<std::collections::BTreeMap<(usize, usize, usize), f32>, String> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|error| format!("--block-lambdas-file 需要 JSON 对象: {error}"))?;
    let object = value
        .as_object()
        .ok_or("--block-lambdas-file 需要 JSON 对象")?;
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

fn parse_unit_patterns(
    raw: &str,
    arity: usize,
    flag: &str,
) -> Result<Vec<crate::scale::UnitPattern>, String> {
    let value: Value =
        serde_json::from_str(raw).map_err(|error| format!("{flag} 需要 JSON 对象: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| format!("{flag} 需要 JSON 对象"))?;
    let mut patterns = Vec::new();
    for (key, value) in object {
        let parts: Vec<&str> = key.split(':').collect();
        if parts.len() != arity {
            return Err(format!("键 {key} 需要 {arity} 段"));
        }
        let mut parsed = Vec::with_capacity(arity);
        for part in parts {
            parsed.push(parse_pattern_part(part).map_err(|error| format!("键 {key}: {error}"))?);
        }
        let lambda = value.as_f64().ok_or_else(|| format!("键 {key} 需要数字"))? as f32;
        patterns.push(crate::scale::UnitPattern {
            parts: parsed,
            lambda,
        });
    }
    Ok(patterns)
}

fn parse_pattern_part(part: &str) -> Result<crate::scale::UnitPart, String> {
    if part == "*" {
        return Ok(crate::scale::UnitPart::Any);
    }
    let mut ranges = Vec::new();
    for piece in part.split('|') {
        let piece = piece.trim();
        if piece.is_empty() {
            return Err(format!("{part} 有空段"));
        }
        if piece == "*" {
            return Err(format!("{part} 不能把 * 与数字混用"));
        }
        if let Some((start, end)) = piece.split_once('-') {
            let start = start
                .parse::<usize>()
                .map_err(|_| format!("{piece} 的起点非法"))?;
            let end = end
                .parse::<usize>()
                .map_err(|_| format!("{piece} 的终点非法"))?;
            if start > end {
                return Err(format!("{piece} 的起点大于终点"));
            }
            ranges.push(start..=end);
        } else {
            let value = piece
                .parse::<usize>()
                .map_err(|_| format!("{piece} 非法"))?;
            ranges.push(value..=value);
        }
    }
    if ranges.is_empty() {
        return Err(format!("{part} 不能为空"));
    }
    Ok(crate::scale::UnitPart::Ranges(ranges))
}

fn render_pattern(parts: &[crate::scale::UnitPart]) -> String {
    parts
        .iter()
        .map(|part| match part {
            crate::scale::UnitPart::Any => "*".to_string(),
            crate::scale::UnitPart::Ranges(ranges) => ranges
                .iter()
                .map(|range| {
                    if range.start() == range.end() {
                        range.start().to_string()
                    } else {
                        format!("{}-{}", range.start(), range.end())
                    }
                })
                .collect::<Vec<_>>()
                .join("|"),
        })
        .collect::<Vec<_>>()
        .join(":")
}
