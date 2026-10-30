//! `trit-net`：三值权重网络的最小 CLI（实验层）。

use std::path::PathBuf;
use std::process::ExitCode;

use trit_core::value::Ternary;
use trit_net::{
    Analysis, DEFAULT_BUDGET, Layer, Model, Outcome, TrainOptions, analyze, format,
    majority_dataset, netsec, train,
};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("format") => format_cmd(&args[1..]),
        Some("train") => train_cmd(&args[1..]),
        Some("flow") => flow_cmd(&args[1..]),
        _ => run(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn format_cmd(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("encode") => {
            let path = args.get(1).ok_or("format encode 需要 MODEL.json")?;
            let text = std::fs::read_to_string(path)
                .map_err(|error| format!("读取模型失败 {path}：{error}"))?;
            let model = Model::from_json(&text).map_err(|error| error.to_string())?;
            let bytes = format::encode(&model).map_err(|error| error.to_string())?;
            let mut obj = serde_json::json!({
                "model": path,
                "bytes": bytes.len(),
                "json_bytes": text.len(),
            });
            match args.iter().position(|arg| arg == "--out") {
                Some(index) => {
                    let out = args.get(index + 1).ok_or("--out 缺少路径")?;
                    std::fs::write(out, &bytes)
                        .map_err(|error| format!("写入 {out} 失败：{error}"))?;
                    obj["out"] = serde_json::Value::String(out.clone());
                }
                None => {
                    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
                    obj["hex"] = serde_json::Value::String(hex);
                }
            }
            println!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
            Ok(())
        }
        Some("decode") => {
            let path = args.get(1).ok_or("format decode 需要 .trn 文件")?;
            let bytes =
                std::fs::read(path).map_err(|error| format!("读取模型失败 {path}：{error}"))?;
            let model = format::decode(&bytes).map_err(|error| error.to_string())?;
            let obj = serde_json::json!({"model": model, "bytes": bytes.len()});
            println!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
            Ok(())
        }
        _ => Err(
            "用法：trit-net format <encode MODEL.json [--out FILE] | decode FILE.trn>".to_string(),
        ),
    }
}

fn flag_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1).cloned())
}

fn train_cmd(args: &[String]) -> Result<(), String> {
    let epochs = flag_value(args, "--epochs").map_or(Ok(300), |raw| {
        raw.parse().map_err(|_| "epochs 不是整数".to_string())
    })?;
    let hidden = flag_value(args, "--hidden").map_or(Ok(6), |raw| {
        raw.parse().map_err(|_| "hidden 不是整数".to_string())
    })?;
    let learning_rate = flag_value(args, "--lr").map_or(Ok(0.3), |raw| {
        raw.parse::<f32>().map_err(|_| "lr 不是数字".to_string())
    })?;
    let inputs = 5;
    let mut model = Model {
        layers: vec![
            Layer {
                inputs,
                outputs: hidden,
                weights: (0..hidden)
                    .map(|row| {
                        (0..inputs)
                            .map(|col| if (row + col) % 3 == 0 { 1 } else { -1 })
                            .collect()
                    })
                    .collect(),
                bias: vec![0; hidden],
            },
            Layer {
                inputs: hidden,
                outputs: 1,
                weights: vec![
                    (0..hidden)
                        .map(|col| if col % 2 == 0 { 1 } else { -1 })
                        .collect(),
                ],
                bias: vec![0],
            },
        ],
    };
    let (samples, labels) = majority_dataset(inputs);
    let report = train(
        &mut model,
        &samples,
        &labels,
        TrainOptions {
            epochs,
            learning_rate,
            seed: 11,
        },
    )
    .map_err(|error| error.to_string())?;
    let bytes = format::encode(&model).map_err(|error| error.to_string())?;
    let obj = serde_json::json!({
        "task": format!("majority of {inputs}"),
        "samples": samples.len(),
        "hidden": hidden,
        "epochs": report.epochs,
        "initial_loss": report.initial_loss,
        "final_loss": report.final_loss,
        "shadow_accuracy": report.shadow_accuracy,
        "ternary_accuracy": report.ternary_accuracy,
        "model_bytes": bytes.len(),
    });
    println!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
    Ok(())
}

struct Args {
    model: PathBuf,
    inputs: String,
    budget: usize,
    json: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        return Err(usage());
    };
    if command != "infer" {
        return Err(format!("未知子命令 {command:?}\n{}", usage()));
    }
    let mut model = None;
    let mut inputs = None;
    let mut budget = DEFAULT_BUDGET;
    let mut json = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--inputs" => {
                inputs = Some(args.next().ok_or("--inputs 缺少参数")?);
            }
            "--budget" => {
                let raw = args.next().ok_or("--budget 缺少参数")?;
                budget = raw
                    .parse()
                    .map_err(|_| format!("--budget 不是整数: {raw}"))?;
            }
            "--json" => json = true,
            value if value.starts_with('-') => return Err(format!("未知参数 {value}")),
            value => model = Some(PathBuf::from(value)),
        }
    }
    Ok(Args {
        model: model.ok_or("缺少模型 JSON 路径")?,
        inputs: inputs.ok_or("缺少 --inputs，例如 --inputs 1,X,0")?,
        budget,
        json,
    })
}

fn usage() -> String {
    "用法：trit-net <infer MODEL.json --inputs 1,X,0 [--budget N] [--json] | format encode|decode | train [--epochs N] | flow PCAP [--model MODEL] [--budget N]>".to_string()
}

fn flow_cmd(args: &[String]) -> Result<(), String> {
    let mut path: Option<String> = None;
    let mut model_path: Option<String> = None;
    let mut budget = DEFAULT_BUDGET;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--model" => {
                index += 1;
                model_path = Some(args.get(index).ok_or("--model 缺少参数")?.clone());
            }
            "--budget" => {
                index += 1;
                let raw = args.get(index).ok_or("--budget 缺少参数")?;
                budget = raw
                    .parse()
                    .map_err(|_| format!("--budget 不是整数: {raw}"))?;
            }
            other if other.starts_with("--") => return Err(format!("未知参数 {other}")),
            other => {
                if path.is_some() {
                    return Err(format!("多余的参数 {other}"));
                }
                path = Some(other.to_string());
            }
        }
        index += 1;
    }
    let path = path.ok_or("flow 需要 PCAP 路径")?;
    let data = std::fs::read(&path).map_err(|error| format!("读取 {path} 失败：{error}"))?;
    let model = match model_path.as_deref() {
        Some(model_path) => {
            let bytes = std::fs::read(model_path)
                .map_err(|error| format!("读取模型失败 {model_path}：{error}"))?;
            let model = if model_path.ends_with(".trn") {
                format::decode(&bytes).map_err(|error| error.to_string())?
            } else {
                let text = String::from_utf8(bytes)
                    .map_err(|_| format!("模型 {model_path} 不是 UTF-8 JSON"))?;
                Model::from_json(&text).map_err(|error| error.to_string())?
            };
            if model.inputs() != netsec::flow::N {
                return Err(format!(
                    "模型输入维数 {} 与特征维数 {} 不一致",
                    model.inputs(),
                    netsec::flow::N
                ));
            }
            Some(model)
        }
        None => None,
    };

    let reader = netsec::pcap::Reader::new(&data).map_err(|error| error.to_string())?;
    let link = reader.link_type();
    let nanos = reader.nanos();
    let mut table = netsec::flow::FlowTable::new();
    let mut packets = 0usize;
    let mut skips: std::collections::BTreeMap<&'static str, usize> =
        std::collections::BTreeMap::new();
    for record in reader {
        let record = record.map_err(|error| error.to_string())?;
        let ts = record.timestamp_seconds(nanos);
        match netsec::packet::decode(link, record.data) {
            Ok(packet) => {
                table.ingest(ts, &packet);
                packets += 1;
            }
            Err(skip) => *skips.entry(skip.as_str()).or_default() += 1,
        }
    }
    let flows = table.finish();
    for flow in &flows {
        let values = flow.features.values();
        let inputs = netsec::features::ternarize(&values, &netsec::features::DEFAULT_RULES);
        let features = netsec::flow::NAMES
            .iter()
            .zip(values.iter())
            .map(|(name, value)| (name.to_string(), serde_json::json!(value)))
            .collect::<serde_json::Map<String, serde_json::Value>>();
        let mut obj = serde_json::json!({
            "src": flow.src().to_string(),
            "dst": flow.dst().to_string(),
            "proto": flow.key.proto,
            "first_ts": flow.first_ts,
            "packets": flow.features.fwd_pkts + flow.features.bwd_pkts,
            "inputs": netsec::features::encode(&inputs),
            "features": features,
        });
        if let Some(model) = &model {
            let decision = netsec::decision::decide(model, &inputs, budget)
                .map_err(|error| error.to_string())?;
            obj["verdict"] = serde_json::json!(decision.verdict.as_str());
            obj["reason"] = serde_json::json!(decision.reason);
            obj["exact"] = serde_json::json!(decision.exact);
            obj["completions"] = serde_json::json!(decision.completions);
            obj["unknown_inputs"] = serde_json::json!(decision.unknown_inputs);
        }
        println!(
            "{}",
            serde_json::to_string(&obj).map_err(|error| format!("JSON 序列化失败：{error}"))?
        );
    }
    let mut summary = format!("报文 {packets} 条，流 {} 条", flows.len());
    if model.is_some() {
        summary.push_str("；规则集 DEFAULT_RULES（未校准）");
    }
    if !skips.is_empty() {
        summary.push_str("；跳过：");
        for (reason, count) in &skips {
            summary.push_str(&format!(" {reason}={count}"));
        }
    }
    eprintln!("{summary}");
    Ok(())
}

fn parse_inputs(raw: &str) -> Result<Vec<Ternary>, String> {
    raw.split(',')
        .map(|item| {
            let mut chars = item.trim().chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Ternary::parse_char(c)
                    .ok_or_else(|| format!("未知输入值 {item:?}（只接受 0/1/X）")),
                _ => Err(format!("未知输入值 {item:?}（只接受 0/1/X）")),
            }
        })
        .collect()
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let text = std::fs::read_to_string(&args.model)
        .map_err(|error| format!("读取模型失败 {}：{error}", args.model.display()))?;
    let model = Model::from_json(&text).map_err(|error| error.to_string())?;
    let inputs = parse_inputs(&args.inputs)?;
    let analysis = analyze(&model, &inputs, args.budget).map_err(|error| error.to_string())?;

    if args.json {
        print_json(&model, &inputs, &analysis);
    } else {
        print_table(&model, &inputs, &analysis);
    }
    Ok(())
}

fn chars(values: &[Ternary]) -> Vec<String> {
    values
        .iter()
        .map(|value| value.as_char().to_string())
        .collect()
}

fn print_json(model: &Model, inputs: &[Ternary], analysis: &Analysis) {
    let outputs = analysis
        .outputs
        .iter()
        .enumerate()
        .map(|(index, outcome)| {
            let (verdict, pos, neg) = match outcome {
                Outcome::Definite(sign) => ("definite", Some(sign.as_char().to_string()), None),
                Outcome::Ambiguous { pos, neg } => (
                    "ambiguous",
                    pos.as_ref().map(|witness| chars(&witness.values).join(",")),
                    neg.as_ref().map(|witness| chars(&witness.values).join(",")),
                ),
                Outcome::BudgetExceeded => ("budget_exceeded", None, None),
            };
            (
                index.to_string(),
                serde_json::json!({
                    "verdict": verdict,
                    "value": if verdict == "definite" { pos.clone() } else { None },
                    "pos_witness": pos,
                    "neg_witness": neg,
                    "conservative": analysis.conservative[index].map(|s| s.as_char().to_string()),
                }),
            )
        })
        .collect::<serde_json::Map<String, serde_json::Value>>();
    let obj = serde_json::json!({
        "layers": model.layers.len(),
        "inputs": chars(inputs),
        "unknown_inputs": analysis.unknown_inputs,
        "exact": analysis.exact,
        "completions": analysis.completions,
        "budget": analysis.budget,
        "outputs": outputs,
    });
    match serde_json::to_string_pretty(&obj) {
        Ok(text) => println!("{text}"),
        Err(error) => eprintln!("JSON 序列化失败：{error}"),
    }
}

fn print_table(model: &Model, inputs: &[Ternary], analysis: &Analysis) {
    println!(
        "三值网络：{} 层，输入 {} 个；未知输入 {} 个，补全 {}（预算 {}）",
        model.layers.len(),
        inputs.len(),
        analysis.unknown_inputs.len(),
        if analysis.exact {
            analysis.completions.to_string()
        } else {
            "超预算".to_string()
        },
        analysis.budget
    );
    for (index, outcome) in analysis.outputs.iter().enumerate() {
        let conservative = match analysis.conservative[index] {
            Some(sign) => sign.as_char().to_string(),
            None => "未定".to_string(),
        };
        match outcome {
            Outcome::Definite(sign) => {
                println!(
                    "  输出 {index} = {}（确定；保守区间 {}）",
                    sign.as_char(),
                    conservative
                );
            }
            Outcome::Ambiguous { pos, neg } => {
                let describe = |witness: &Option<trit_net::Witness>| match witness {
                    Some(witness) => chars(&witness.values).join(","),
                    None => "无".to_string(),
                };
                println!(
                    "  输出 {index} = 未定（+1 见证 {}；-1 见证 {}；保守区间 {}）",
                    describe(pos),
                    describe(neg),
                    conservative
                );
            }
            Outcome::BudgetExceeded => {
                println!("  输出 {index} = 未解析（未知输入超过枚举预算）");
            }
        }
    }
    println!(
        "\n确定 {} / 共 {}；保守区间只敢确定 {}",
        analysis.definite_count(),
        analysis.outputs.len(),
        analysis.conservative_definite_count()
    );
}
