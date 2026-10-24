//! 三进制产物的红队/加固检查器。
//!
//! **边界**：只读本机、本仓自己的产物（TRB2 blob、`--emit-verilog` 导出的网表），
//! 不发网络请求、不做目标发现、不做规避检测。目的是把"我们的格式有哪些可被利用的性质"
//! 变成可复算的判据，而不是提供攻击别人的手段。
//!
//! 三个子命令对应三类攻击面：
//! - `encode`：TRB2 非规范编码（同一张表多个字节串）+ 隐写容量
//! - `crc`：CRC-3 的检出矩阵与最小不可检重量
//! - `rail`：双导轨 `(1,1)` 非法态的可达性、粘滞与 fail-open

mod binary;
mod blob;
mod chain;
mod crc;
mod fuzz;
mod rail;
mod sidechannel;

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "trit-redteam", about = "三进制产物红队检查器（只打自家格式）")]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// 有发现就以 1 退出（可直接当 CI 门用）
    #[arg(long, global = true)]
    strict: bool,
}

#[derive(Subcommand)]
enum Command {
    /// TRB2 blob：非规范编码（填充位可塑）与隐写容量
    Encode {
        /// `truth.tritbin` 路径
        blob: PathBuf,
    },
    /// CRC-3：逐重量检出矩阵 + 最小不可检重量
    Crc {
        /// 载荷 trit 数
        #[arg(long, default_value_t = 9)]
        trits: usize,
        /// 最多枚举到多少重量
        #[arg(long, default_value_t = 5)]
        max_weight: usize,
    },
    /// 双导轨 design.v：非法态与 fail-open 分析
    Rail {
        /// `design.v` 路径
        design: PathBuf,
    },
    /// TRB2 差分 fuzz：Rust 解析器 vs C 运行时（需要 C 编译器）
    Fuzz {
        /// 起始 blob（通常是 `truth.tritbin`）
        blob: PathBuf,
        /// 变异用例数
        #[arg(long, default_value_t = 500)]
        iterations: usize,
        /// 语料种子（同种子同语料）
        #[arg(long, default_value_t = 1)]
        seed: u64,
    },
    /// `--manifest jobs.json` 的执行面静态检查
    Chain {
        /// `jobs.json` 路径
        manifest: PathBuf,
    },
    /// 双导轨一阶侧信道：汉明重量 → Unknown 泄漏量（bit）
    Sidechannel {
        /// `design.v` 路径
        design: PathBuf,
    },
    /// 本仓产物的硬化与泄密检查（PE / ELF）
    Binary {
        /// 可执行文件路径
        file: PathBuf,
    },
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn fnv64(bytes: &[u8]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn run_encode(path: &PathBuf) -> Result<(serde_json::Value, bool)> {
    let bytes = fs::read(path).with_context(|| format!("读 {} 失败", path.display()))?;
    let parsed = blob::parse(&bytes)?;
    let canonical = blob::is_canonical(&bytes)?;
    let unused = blob::unused_trits(&bytes)?;
    let variants = blob::padding_variants(&bytes)?;
    let distinct = variants.iter().filter(|variant| *variant != &bytes).count();
    let covert_bits = (variants.len() as f64).log2();
    let samples: Vec<serde_json::Value> = variants
        .iter()
        .filter(|variant| *variant != &bytes)
        .take(3)
        .map(|variant| {
            serde_json::json!({
                "hex": hex(variant),
                "fnv64": fnv64(variant),
                "same_table": blob::parse(variant).map(|other| other.values == parsed.values).unwrap_or(false),
            })
        })
        .collect();
    let report = serde_json::json!({
        "tool": "trit-redteam",
        "check": "trb2-noncanonical",
        "blob": {
            "bytes": bytes.len(),
            "trits": parsed.values.len(),
            "nvars": parsed.nvars,
            "algebra": parsed.algebra,
            "fnv64": fnv64(&bytes),
        },
        "padding": {
            "unused_trits": unused,
            "canonical": canonical,
            "equivalent_encodings": variants.len(),
            "other_encodings": distinct,
            "covert_bits": (covert_bits * 100.0).round() / 100.0,
        },
        "samples": samples,
        "impact": if distinct > 0 {
            "同一张真值表存在多个字节串：内容哈希不等于语义身份，闲置槽位可夹带数据"
        } else {
            "载荷正好占满字节，没有多余编码空间"
        },
    });
    Ok((report, distinct > 0))
}

fn run_crc(trits: usize, max_weight: usize) -> Result<(serde_json::Value, bool)> {
    let scan = crc::scan(trits, max_weight);
    let rows: Vec<serde_json::Value> = scan
        .rows
        .iter()
        .map(|row| {
            serde_json::json!({
                "weight": row.weight,
                "tested": row.tested,
                "undetected": row.undetected,
                "detection_rate": if row.tested == 0 { 1.0 } else { 1.0 - row.undetected as f64 / row.tested as f64 },
                "example": row.example.as_ref().map(crc::Example::to_json),
            })
        })
        .collect();
    let min = scan
        .min_undetected()
        .map(|row| serde_json::json!({ "weight": row.weight, "example": row.example.as_ref().map(crc::Example::to_json) }));
    let finding = scan.min_undetected().is_some_and(|row| row.weight <= 3);
    let report = serde_json::json!({
        "tool": "trit-redteam",
        "check": "crc3-detection",
        "trits": scan.length,
        "rows": rows,
        "detects_all_up_to_weight_3": scan.detects_all_up_to(3),
        "min_undetected": min,
        "note": "GF(3) 线性码：错误模式能否检出与载荷内容无关；冗余 3 trit ⇒ Singleton 上界 d ≤ 4",
    });
    Ok((report, finding))
}

fn run_rail(path: &PathBuf) -> Result<(serde_json::Value, bool)> {
    let text = fs::read_to_string(path).with_context(|| format!("读 {} 失败", path.display()))?;
    let report = rail::analyze(&text)?;
    let finding = rail::has_findings(&report);
    let value = serde_json::json!({
        "tool": "trit-redteam",
        "check": "two-rail-illegal-state",
        "signals": report.signals,
        "combinations": report.combinations,
        "legal_inputs": report.legal_inputs,
        "illegal_inputs": report.illegal_inputs,
        "legal_output_invalid": report.legal_output_invalid,
        "illegal_output_true_by_pos": report.illegal_output_true_by_pos,
        "illegal_output_true": report.illegal_output_true,
        "illegal_output_invalid": report.illegal_output_invalid,
        "all_illegal_output": report.all_illegal_output,
        "examples": report.examples,
        "note": "(1,1) 不是合法值；只看 pos 的消费端会把 (1,1) 读成 True ⇒ fail-open",
    });
    Ok((value, finding))
}

fn run_fuzz(path: &PathBuf, iterations: usize, seed: u64) -> Result<(serde_json::Value, bool)> {
    let bytes = fs::read(path).with_context(|| format!("读 {} 失败", path.display()))?;
    fuzz::run(&bytes, iterations, seed)
}

fn run_chain(path: &PathBuf) -> Result<(serde_json::Value, bool)> {
    let text = fs::read_to_string(path).with_context(|| format!("读 {} 失败", path.display()))?;
    chain::run(&text)
}

fn run_sidechannel(path: &PathBuf) -> Result<(serde_json::Value, bool)> {
    let text = fs::read_to_string(path).with_context(|| format!("读 {} 失败", path.display()))?;
    let report = sidechannel::analyze(&text)?;
    let finding = sidechannel::has_findings(&report);
    Ok((sidechannel::to_json(&report), finding))
}

fn run_binary(path: &PathBuf) -> Result<(serde_json::Value, bool)> {
    let bytes = fs::read(path).with_context(|| format!("读 {} 失败", path.display()))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("?");
    let report = binary::run(&bytes, name)?;
    let finding = binary::has_findings(&report);
    Ok((report, finding))
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let result = match &cli.command {
        Command::Encode { blob } => run_encode(blob),
        Command::Crc { trits, max_weight } => run_crc(*trits, *max_weight),
        Command::Rail { design } => run_rail(design),
        Command::Fuzz {
            blob,
            iterations,
            seed,
        } => run_fuzz(blob, *iterations, *seed),
        Command::Chain { manifest } => run_chain(manifest),
        Command::Sidechannel { design } => run_sidechannel(design),
        Command::Binary { file } => run_binary(file),
    };
    match result {
        Ok((report, finding)) => {
            println!("{report}");
            if cli.strict && finding {
                return std::process::ExitCode::FAILURE;
            }
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
