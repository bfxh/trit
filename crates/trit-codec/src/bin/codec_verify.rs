//! 三进制专项评估：`trit-codec` 的「编码可逆 + 逐位一致」独立复核。
//!
//! 复核不是把编码器再调一遍：编解码同错也能自洽往返。这里每条检查都另写一份最小
//! 独立实现（位级打包、平衡三进制解析、mod 3 差分还原、稀疏重建），与库输出逐位比对，
//! 外加纠错/检错不变式与非法输入拒绝。
//!
//! 用法：
//!
//! ```text
//! cargo run -p trit-codec --bin codec_verify [-- --json] [--quick] [--seed 0x...]
//! ```
//!
//! 退出码：0 全绿；2 有失败；64 参数错误。口径与读数见 `docs/research/TERNARY-AUDIT.md`。

#[path = "codec_verify/checks.rs"]
mod checks;
#[path = "codec_verify/corpus.rs"]
mod corpus;
#[path = "codec_verify/report.rs"]
mod report;

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut json = false;
    let mut quick = false;
    let mut seed = corpus::SEED_DEFAULT;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "--json" => {
                json = true;
                index += 1;
            }
            "--quick" => {
                quick = true;
                index += 1;
            }
            "--seed" => {
                let Some(raw) = args.get(index + 1) else {
                    return usage();
                };
                let Some(parsed) = parse_seed(raw) else {
                    eprintln!("--seed 需要十进制或 0x 前缀十六进制");
                    return ExitCode::from(64);
                };
                seed = parsed;
                index += 2;
            }
            other => {
                eprintln!("未知参数 {other}");
                return usage();
            }
        }
    }

    let samples = corpus::samples(quick, seed);
    let (count, trits) = corpus::stats(&samples);
    let mut report = report::Report::new(
        "trit-codec-verify",
        "三进制专项评估：trit-codec 编码可逆与逐位一致复核",
        seed,
        quick,
    );
    report.samples = count;
    report.trits = trits;
    checks::run(&mut report, &samples);

    if json {
        println!("{}", report.to_json());
    } else {
        print!("{}", report.to_text());
    }

    if report.ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(2)
    }
}

fn parse_seed(raw: &str) -> Option<u64> {
    match raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => raw.parse().ok(),
    }
}

fn usage() -> ExitCode {
    eprintln!("用法：codec_verify [--json] [--quick] [--seed <u64|0xhex>]");
    ExitCode::from(64)
}
