//! `trit-attn`：双视角等价性、三值关联记忆与可训练注意力块的读数入口。

use std::process::ExitCode;

use trit_attn::RecallTrainOptions;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("demo") => demo(&args[1..]),
        Some("train") => train(&args[1..]),
        _ => Err("用法：trit-attn demo|train\n  demo  [--pairs N] [--dim D] [--seed N]\n  train [--epochs N] [--lr F] [--hidden N] [--seed N]".to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn number(args: &[String], name: &str, default: usize) -> Result<usize, String> {
    match args.iter().position(|arg| arg == name) {
        Some(index) => args
            .get(index + 1)
            .ok_or_else(|| format!("{name} 缺少参数"))?
            .parse()
            .map_err(|_| format!("{name} 不是整数")),
        None => Ok(default),
    }
}

fn float(args: &[String], name: &str, default: f32) -> Result<f32, String> {
    match args.iter().position(|arg| arg == name) {
        Some(index) => args
            .get(index + 1)
            .ok_or_else(|| format!("{name} 缺少参数"))?
            .parse()
            .map_err(|_| format!("{name} 不是数字")),
        None => Ok(default),
    }
}

fn demo(args: &[String]) -> Result<(), String> {
    let pairs = number(args, "--pairs", 32)?;
    let dim = number(args, "--dim", 16)?;
    let seed = number(args, "--seed", 7)? as u64;
    if pairs == 0 || dim == 0 {
        return Err("pairs/dim 必须大于 0".to_string());
    }
    let report = trit_attn::recall_experiment(seed, pairs, dim);
    let obj = serde_json::json!({
        "pairs": report.pairs,
        "dim": report.dim,
        "hidden_width_equals_sequence_length": true,
        "attention_ffn_max_diff": report.attention_ffn_max_diff,
        "recall": {
            "ternary_integer_memory": report.ternary_accuracy,
            "float_memory_energy_unmatched": report.float_accuracy,
            "softmax_attention": report.attention_accuracy,
        },
    });
    println!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
    Ok(())
}

fn train(args: &[String]) -> Result<(), String> {
    let epochs = number(args, "--epochs", 1200)?;
    let hidden = number(args, "--hidden", 24)?;
    let seed = number(args, "--seed", 7)? as u64;
    let lr = float(args, "--lr", 0.3)?;
    if epochs == 0 || hidden == 0 {
        return Err("epochs/hidden 必须大于 0".to_string());
    }
    let report = trit_attn::train_recall(RecallTrainOptions {
        epochs,
        learning_rate: lr,
        hidden,
        seed,
        ..RecallTrainOptions::default()
    });
    let obj = serde_json::json!({
        "task": "associative_recall",
        "samples": report.samples,
        "dim": report.dim,
        "seq_len": report.seq_len,
        "hidden": report.hidden,
        "epochs": report.epochs,
        "initial_loss": report.initial_loss,
        "final_loss": report.final_loss,
        "recall_accuracy": report.recall_accuracy,
        "attention_on_value_position": report.attention_on_value_position,
    });
    println!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
    Ok(())
}
