//! `gguf-to-mod`：GGUF（未量化权重）→ MOD（TTX2 模型容器）。
//!
//! ```text
//! cargo run -p trit-texture --bin gguf-to-mod -- in.gguf out.ttx [选项]
//!   --threshold <f32>      |w| ≤ 阈值 → 0（默认 0 = 纯符号）
//!   --tensor <name>        只导入该张量（可重复；按给出顺序成层）
//!   --max-bytes <n>        文件字节上限（默认 256 MiB）
//! ```
//!
//! 只支持未量化张量（F32/F16/BF16）；量化类型显式拒绝，理由见 `gguf` 模块文档。
#![forbid(unsafe_code)]

use std::path::Path;
use std::process::ExitCode;

use trit_texture::gguf::{ImportOptions, import_gguf};
use trit_texture::ttx2::{TtxDocument, to_ttx2};

const USAGE: &str =
    "用法：gguf-to-mod <输入.gguf> <输出.ttx> [--threshold F] [--tensor NAME ...] [--max-bytes N]";

fn main() -> ExitCode {
    match run() {
        Ok(summary) => {
            println!("{summary}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("gguf-to-mod：{message}\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<String, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut positional = Vec::new();
    let mut options = ImportOptions::default();
    let mut tensors: Vec<String> = Vec::new();
    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "--threshold" => {
                let raw = args.get(index + 1).ok_or("--threshold 缺少数值")?;
                options.zero_threshold = raw
                    .parse()
                    .map_err(|_| format!("--threshold 不是数字：{raw}"))?;
                index += 2;
            }
            "--tensor" => {
                tensors.push(args.get(index + 1).ok_or("--tensor 缺少名字")?.clone());
                index += 2;
            }
            "--max-bytes" => {
                let raw = args.get(index + 1).ok_or("--max-bytes 缺少数值")?;
                options.max_bytes = raw
                    .parse()
                    .map_err(|_| format!("--max-bytes 不是数字：{raw}"))?;
                index += 2;
            }
            flag if flag.starts_with("--") => return Err(format!("未知选项 {flag}")),
            value => {
                positional.push(value.to_string());
                index += 1;
            }
        }
    }
    if positional.len() != 2 {
        return Err("需要恰好两个位置参数（输入、输出）".to_string());
    }
    if !tensors.is_empty() {
        options.tensor_names = Some(tensors);
    }
    let input = Path::new(&positional[0]);
    let output = Path::new(&positional[1]);
    let bytes = std::fs::read(input).map_err(|error| format!("读取 {input:?} 失败：{error}"))?;
    let model = import_gguf(&bytes, &options).map_err(|error| error.to_string())?;
    let container =
        to_ttx2(&TtxDocument::Model(model.clone())).map_err(|error| error.to_string())?;
    std::fs::write(output, &container).map_err(|error| format!("写入 {output:?} 失败：{error}"))?;
    let weights: usize = model.layers.iter().map(|layer| layer.weights.len()).sum();
    let zeros = model
        .layers
        .iter()
        .map(|layer| layer.weights.iter().filter(|value| **value == 0).count())
        .sum::<usize>();
    Ok(format!(
        "已写出 {}：{} 层 / {} 权重 / 零占比 {:.1}% / {} 字节",
        output.display(),
        model.layers.len(),
        weights,
        if weights == 0 {
            0.0
        } else {
            zeros as f64 / weights as f64 * 100.0
        },
        container.len()
    ))
}
