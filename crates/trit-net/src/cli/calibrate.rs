//! `trit-net calibrate` 子命令：CSV 数据集 → 分档校准 → 三值训练 → 计分与攻击面读数。
//!
//! 从 `main.rs` 拆出来：它是这个 CLI 里唯一要跑完整训练回路的一条命令，
//! 参数面（rows/epochs/hidden/bands/train-fraction）和读数打印都自成一件事。
//! 子命令仍由 `main.rs` 的分派表调用，行为与公开面不变。

use trit_net::netsec;

pub(crate) fn calibrate_cmd(args: &[String]) -> Result<(), String> {
    let mut path: Option<String> = None;
    let mut rows = 200_000usize;
    let mut epochs = 120usize;
    let mut hidden = 8usize;
    let mut learning_rate = 0.3f32;
    let mut features = 8usize;
    let mut select_features = true;
    let mut test_path: Option<String> = None;
    let mut bands = vec![0.0, 0.1, 0.2, 0.3, 0.4];
    let mut fraction = 0.75f64;
    let mut json = false;
    let mut batch_size = 0usize;
    let mut benign_tail: Option<f64> = None;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].clone();
        match flag.as_str() {
            "--no-select" => select_features = false,
            "--batch" => {
                index += 1;
                let raw = args.get(index).ok_or_else(|| format!("{flag} 缺少参数"))?;
                batch_size = raw.parse().map_err(|_| "--batch 不是整数".to_string())?;
            }
            "--benign-tail" => {
                index += 1;
                let raw = args.get(index).ok_or_else(|| format!("{flag} 缺少参数"))?;
                benign_tail = Some(raw.parse().map_err(|_| "--benign-tail 不是数字".to_string())?);
            }
            "--rows" | "--epochs" | "--hidden" | "--train-fraction" | "--bands" | "--lr"
            | "--features" | "--test" => {
                index += 1;
                let raw = args.get(index).ok_or_else(|| format!("{flag} 缺少参数"))?;
                match flag.as_str() {
                    "--rows" => rows = raw.parse().map_err(|_| "--rows 不是整数".to_string())?,
                    "--epochs" => {
                        epochs = raw.parse().map_err(|_| "--epochs 不是整数".to_string())?
                    }
                    "--hidden" => {
                        hidden = raw.parse().map_err(|_| "--hidden 不是整数".to_string())?
                    }
                    "--lr" => {
                        learning_rate = raw.parse().map_err(|_| "--lr 不是数字".to_string())?
                    }
                    "--train-fraction" => {
                        fraction = raw
                            .parse()
                            .map_err(|_| "--train-fraction 不是数字".to_string())?
                    }
                    "--features" => {
                        features = raw.parse().map_err(|_| "--features 不是整数".to_string())?
                    }
                    "--test" => test_path = Some(raw.clone()),
                    _ => {
                        bands = raw
                            .split(',')
                            .map(|item| item.trim().parse::<f64>())
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(|_| format!("--bands 需要逗号分隔的数字: {raw}"))?
                    }
                }
            }
            "--json" => json = true,
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
    let path = path.ok_or("calibrate 需要 CICIDS CSV 路径")?;
    let text =
        std::fs::read_to_string(&path).map_err(|error| format!("读取 {path} 失败：{error}"))?;
    let dataset = netsec::dataset::parse(&text, rows).map_err(|error| error.to_string())?;
    if dataset.samples.is_empty() {
        return Err(format!(
            "CSV 里没有可用样本（行 {}，跳过 {}）",
            dataset.rows, dataset.skipped
        ));
    }
    let config = netsec::calibrate::Config {
        bands,
        epochs,
        hidden,
        seed: 0x5eed_1234,
        train_fraction: fraction,
        balance: true,
        learning_rate,
        features,
        select_features,
        batch_size,
        benign_tail,
    };
    let (reports, selected) = match test_path.as_deref() {
        Some(test_path) => {
            let text = std::fs::read_to_string(test_path)
                .map_err(|error| format!("读取 {test_path} 失败：{error}"))?;
            let test = netsec::dataset::parse(&text, rows).map_err(|error| error.to_string())?;
            println!(
                "训练 {} 样本 → 测试 {} 样本（跨集）",
                dataset.samples.len(),
                test.samples.len()
            );
            netsec::calibrate::run_split(&dataset.samples, &test.samples, &config)?
        }
        None => netsec::calibrate::run(&dataset.samples, &config)?,
    };
    let picks: Vec<String> = selected
        .iter()
        .map(|picked| format!("{}({:+.3})", picked.name(), picked.correlation))
        .collect();
    if !json {
        println!("选定特征 {} 个：{}", selected.len(), picks.join(", "));
    }
    if json {
        let bands: Vec<serde_json::Value> = reports
            .iter()
            .map(|report| {
                serde_json::json!({
                    "band": report.band,
                    "low_q": report.low_q,
                    "high_q": report.high_q,
                    "train": report.train,
                    "test": report.test,
                    "attack": report.attack,
                    "coverage": report.coverage,
                    "abstain": report.abstain,
                    "fpr": report.fpr,
                    "tpr": report.tpr,
                    "precision": report.precision,
                    "accuracy": report.accuracy,
                    "tp": report.tp,
                    "fp": report.fp,
                    "tn": report.tn,
                    "fn": report.fn_,
                    "attack_abstain": report.attack_abstain,
                    "benign_abstain": report.benign_abstain,
                })
            })
            .collect();
        let obj = serde_json::json!({
            "rows": dataset.rows,
            "samples": dataset.samples.len(),
            "skipped": dataset.skipped,
            "attacks": dataset.attacks,
            "epochs": config.epochs,
            "hidden": config.hidden,
            "train_fraction": config.train_fraction,
            "seed": config.seed,
            "selected": selected.iter().map(|picked| serde_json::json!({
                "feature": picked.name(),
                "bias": match picked.bias {
                    netsec::features::Bias::High => "high",
                    netsec::features::Bias::Low => "low",
                },
                "correlation": picked.correlation,
            })).collect::<Vec<_>>(),
            "bands": bands,
        });
        println!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
        return Ok(());
    }
    println!(
        "数据集：行 {}，样本 {}，跳过 {}，攻击 {}（{:.2}%）；epochs {}，hidden {}",
        dataset.rows,
        dataset.samples.len(),
        dataset.skipped,
        dataset.attacks,
        100.0 * dataset.attacks as f64 / dataset.samples.len() as f64,
        config.epochs,
        config.hidden
    );
    println!(
        "{:>5}  {:>11}  {:>8}  {:>8}  {:>8}  {:>8}  {:>9}  {:>8}  {:>7}  {:>7}",
        "band",
        "quantile",
        "coverage",
        "abstain",
        "FPR",
        "TPR",
        "precision",
        "accuracy",
        "atk-abs",
        "ben-abs"
    );
    for report in &reports {
        println!(
            "{:>5.2}  [{:.2},{:.2}]  {:>8.4}  {:>8.4}  {:>8.4}  {:>8.4}  {:>9.4}  {:>8.4}  {:>7.4}  {:>7.4}  {:>9.5}  {:>8.4}  {:>8.4}",
            report.band,
            report.low_q,
            report.high_q,
            report.coverage,
            report.abstain,
            report.fpr,
            report.tpr,
            report.precision,
            report.accuracy,
            report.attack_abstain,
            report.benign_abstain,
            report.train_loss,
            report.train_accuracy,
            report.shadow_accuracy
        );
    }
    if let Some(last) = reports.last() {
        println!(
            "测试集计分（band={:.2}）：tp {} / fp {} / tn {} / fn {}；攻击 {} 条",
            last.band, last.tp, last.fp, last.tn, last.fn_, last.attack
        );
    }
    Ok(())
}
