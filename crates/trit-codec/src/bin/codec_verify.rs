//! 三进制专项评估：`trit-codec` 的「编码可逆 + 逐位一致 + 有损量化」独立复核。
//!
//! 复核不是把编码器再调一遍：编解码同错也能自洽往返。这里每条检查都另写一份最小
//! 独立实现（位级打包、平衡三进制解析、mod 3 差分还原、稀疏重建、真值表代算），
//! 与库输出逐位比对，外加纠错/检错不变式与非法输入拒绝。
//!
//! 用法：
//!
//! ```text
//! cargo run -p trit-codec --bin codec_verify [-- --json] [--quick] [--seed 0x...]
//!     [--corpus <examples 输出目录>]
//! ```
//!
//! `--corpus` 吃 `cargo run -p trit-codec --example ternary_corpus -- <dir>` 产出的
//! `truth.csv` / `balanced.csv`，把跨实现差分接上消费者。
//! 退出码：0 全绿；2 有失败；64 参数/语料错误。口径与读数见 `docs/research/TERNARY-AUDIT.md`。

#[path = "codec_verify/args.rs"]
mod args;
#[path = "codec_verify/checks.rs"]
mod checks;
#[path = "codec_verify/corpus.rs"]
mod corpus;
#[path = "codec_verify/external.rs"]
mod external;
#[path = "codec_verify/quantize.rs"]
mod quantize;
#[path = "codec_verify/report.rs"]
mod report;

use std::process::ExitCode;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let opts = match args::parse(&argv) {
        Ok(opts) => opts,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(64);
        }
    };

    let samples = corpus::samples(opts.quick, opts.seed);
    let (count, trits) = corpus::stats(&samples);
    let mut report = report::Report::new(
        "trit-codec-verify",
        "三进制专项评估：trit-codec 编码可逆、逐位一致与有损量化复核",
        opts.seed,
        opts.quick,
    );
    report.samples = count;
    report.trits = trits;
    checks::run(&mut report, &samples);
    quantize::run(&mut report, opts.seed, opts.quick);
    if let Some(dir) = &opts.corpus
        && let Err(message) = external::run(&mut report, dir)
    {
        eprintln!("{message}");
        return ExitCode::from(64);
    }

    if opts.json {
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
