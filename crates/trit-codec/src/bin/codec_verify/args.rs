//! `codec_verify` 的命令行解析：保持主函数只做编排。

use std::path::PathBuf;

use super::corpus;

pub struct Options {
    pub json: bool,
    pub quick: bool,
    pub seed: u64,
    pub corpus: Option<PathBuf>,
}

pub const USAGE: &str =
    "用法：codec_verify [--json] [--quick] [--seed <u64|0xhex>] [--corpus <examples 目录>]";

pub fn parse(argv: &[String]) -> Result<Options, String> {
    let mut opts = Options {
        json: false,
        quick: false,
        seed: corpus::SEED_DEFAULT,
        corpus: None,
    };
    let mut index = 0usize;
    while index < argv.len() {
        match argv[index].as_str() {
            "--json" => {
                opts.json = true;
                index += 1;
            }
            "--quick" => {
                opts.quick = true;
                index += 1;
            }
            "--seed" => {
                let raw = argv.get(index + 1).ok_or(USAGE)?;
                opts.seed = parse_seed(raw).ok_or("--seed 需要十进制或 0x 前缀十六进制")?;
                index += 2;
            }
            "--corpus" => {
                let raw = argv.get(index + 1).ok_or(USAGE)?;
                opts.corpus = Some(PathBuf::from(raw));
                index += 2;
            }
            other => return Err(format!("未知参数 {other}\n{USAGE}")),
        }
    }
    Ok(opts)
}

fn parse_seed(raw: &str) -> Option<u64> {
    match raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => raw.parse().ok(),
    }
}
