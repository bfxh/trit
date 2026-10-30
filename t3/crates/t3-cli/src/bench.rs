//! Measurement, on this machine, against a baseline.
//!
//! Every number this subcommand prints is produced by running both the optimised
//! path and its scalar reference over the same buffer in the same process, and
//! reporting the ratio. There is no table of "expected" numbers anywhere in the
//! repository: if the code changes, the output changes.
//!
//! That is a deliberate constraint. A benchmark that reports only the fast path is a
//! marketing artefact; the interesting question is always *relative to what*, and the
//! honest answer here is "relative to the obvious implementation, on this CPU, with
//! this data distribution" — including the cases where the optimisation buys nothing.

use std::time::Instant;

use t3_core::{quantile_of, FactId, Tri};
use t3_kernel::entropy::{histogram4, histogram8, histogram_naive};

use crate::report::human_bytes;
use crate::scan;

/// Buffer size for the micro benchmarks.
const BUFFER: usize = 32 * 1024 * 1024;

/// Runs `t3 bench`.
pub fn run(args: &[String]) -> Result<i32, String> {
    let mut dir: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dir" => {
                i += 1;
                dir = Some(args.get(i).ok_or("--dir 需要一个目录")?.to_string());
            }
            other => return Err(format!("未知选项 `{other}`")),
        }
        i += 1;
    }

    println!("t3 bench — 本机实测，全部对比标量基线");
    println!();
    println!("机器      {} · {} 线程", std::env::consts::ARCH, threads());
    println!(
        "指令集    AVX2 {}",
        if avx2_available() {
            "可用"
        } else {
            "不可用"
        }
    );
    println!(
        "缓冲      {} 偏斜 + {} 均匀",
        human_bytes(BUFFER as u64),
        human_bytes(BUFFER as u64)
    );
    println!();

    let skewed = skewed_buffer(BUFFER, 3);
    let noise = noise_buffer(BUFFER);

    println!("熵内核，单位 GiB/s（越大越好）");
    println!("  {:<20}{:>9}{:>9}{:>9}", "", "偏斜", "均匀", "偏斜倍数");
    // Reduce to a checksum *inside* the closure. Returning the 1 KiB histogram and
    // black-boxing it copies the whole array on every iteration, which costs more than
    // the difference being measured and swamps it in noise.
    let n_skewed = measure(|| checksum(&histogram_naive(&skewed)));
    let n_noise = measure(|| checksum(&histogram_naive(&noise)));
    let four_skewed = measure(|| checksum(&histogram4(&skewed)));
    let four_noise = measure(|| checksum(&histogram4(&noise)));
    let eight_skewed = measure(|| checksum(&histogram8(&skewed)));
    let eight_noise = measure(|| checksum(&histogram8(&noise)));
    println!(
        "  {:<20}{:>9.2}{:>9.2}{:>9.2}",
        "朴素 1 表", n_skewed, n_noise, 1.0
    );
    println!(
        "  {:<20}{:>9.2}{:>9.2}{:>9.2}   ← 默认",
        "交错 4 表",
        four_skewed,
        four_noise,
        four_skewed / n_skewed
    );
    println!(
        "  {:<20}{:>9.2}{:>9.2}{:>9.2}",
        "交错 8 表",
        eight_skewed,
        eight_noise,
        eight_skewed / n_skewed
    );
    println!(
        "  4 表是默认：偏斜数据上 {:.1}×，均匀数据上 {:.2}×。8 表与 4 表差值在噪声内，却要双倍 L1。",
        four_skewed / n_skewed,
        four_noise / n_noise
    );
    println!("  加速来自打破存储依赖链；而加壳节区恰恰是均匀数据，收益最小。");
    println!();

    // Byte search. Both sides must touch every byte, otherwise the comparison measures
    // how early the needle was found rather than how fast the loop runs.
    let haystack = noise_buffer(BUFFER);
    let scalar_count = measure(|| {
        let mut n = 0usize;
        for needle in [0x00u8, 0x01, 0x7F, 0xFE] {
            n += haystack.iter().filter(|&&b| b == needle).count();
        }
        n
    });
    let vector_count = measure(|| {
        let mut n = 0usize;
        for needle in [0x00u8, 0x01, 0x7F, 0xFE] {
            n += t3_kernel::bytes::count_byte(&haystack, needle);
        }
        n
    });
    println!("字节统计（全缓冲扫描），单位 GiB/s");
    println!("  {:<20}{:>9.2}", "标量逐字节", scalar_count);
    println!(
        "  {:<20}{:>9.2}   ({:.1}×)",
        "SIMD (AVX2/SSE2)",
        vector_count,
        vector_count / scalar_count
    );
    println!();

    match dir {
        Some(root) => corpus_report(&root),
        None => {
            println!("用 --dir <目录> 追加端到端吞吐与阈值标定实测。");
        }
    }
    Ok(0)
}

fn corpus_report(root: &str) {
    let (mut files, problems) = scan::collect(&[root.to_string()], 512 * 1024 * 1024);
    files.sort();
    files.dedup();
    if files.is_empty() {
        println!("目录内没有可用文件: {root}");
        return;
    }

    let engine = t3_core::rules::default_engine();
    let rules = t3_sig::builtin_rules();

    // Analysis throughput isolated from I/O: everything below reads from disk, and on
    // this machine that dominates. This measures the part the tool actually controls.
    if let Some(biggest) = files
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok().map(|m| (m.len(), p)))
        .max_by_key(|(len, _)| *len)
        .map(|(_, p)| p)
    {
        if let Ok(buffer) = std::fs::read(biggest) {
            let repeats = 40;
            let started = Instant::now();
            for _ in 0..repeats {
                let analysis = t3_format::analyze(&buffer);
                let _ = t3_sig::evaluate(rules, &analysis.regions);
            }
            let secs = started.elapsed().as_secs_f64().max(1e-9);
            let gbs =
                (buffer.len() as f64 * f64::from(repeats)) / secs / (1024.0 * 1024.0 * 1024.0);
            println!("纯内存分析（解析 + 特征 + 签名），单线程");
            println!(
                "  {:.2} GiB/s · 样本 {} 重复 {} 次 · {}",
                gbs,
                human_bytes(buffer.len() as u64),
                repeats,
                crate::report::human_duration(started.elapsed())
            );
            println!();
        }
    }

    // Warm the page cache so the numbers describe analysis, not first-touch I/O.
    for path in files.iter().take(files.len().min(64)) {
        if let Ok(bytes) = std::fs::read(path) {
            let _ = t3_format::analyze(&bytes);
        }
    }

    // Single-threaded analysis.
    let mut records = Vec::with_capacity(files.len());
    let mut analyze_us = 0u64;
    let mut total_bytes = 0u64;
    let single_timer = Instant::now();
    for path in &files {
        let record = scan::analyze_one(path, &engine, Some(rules));
        analyze_us += record.elapsed_us;
        total_bytes += record.size;
        records.push(record);
    }
    let single = single_timer.elapsed();

    let secs = single.as_secs_f64().max(1e-9);
    println!("端到端，单线程（页面缓存已预热）");
    println!(
        "  {} 个文件 · {} · {} · {:.0} 文件/s · {:.2} GiB/s",
        records.len(),
        human_bytes(total_bytes),
        crate::report::human_duration(single),
        records.len() as f64 / secs,
        total_bytes as f64 / secs / (1024.0 * 1024.0 * 1024.0)
    );
    println!(
        "  平均每文件 {:.1} μs（含读取与分析）",
        analyze_us as f64 / records.len().max(1) as f64
    );
    if !problems.is_empty() {
        println!("  跳过 {} 项", problems.len());
    }
    println!();

    // Calibration: the quantiles that justify the bands.
    let mut section_entropy: Vec<f32> = Vec::new();
    let mut file_entropy: Vec<f32> = Vec::new();
    let mut overlay_entropy: Vec<f32> = Vec::new();
    let mut imports: Vec<f32> = Vec::new();
    let mut fired: Vec<(FactId, usize)> = Vec::new();
    let mut unknown: Vec<(FactId, usize)> = Vec::new();

    for record in &records {
        // Calibration is per container: mixing `.evtx` logs into the entropy distribution
        // moves every quantile and produces a threshold nobody can justify.
        if record.kind != "pe" {
            continue;
        }
        if let Some(v) = record.facts.num(FactId::SectionHighEntropy) {
            section_entropy.push(v);
        }
        if let Some(v) = record.facts.num(FactId::FileHighEntropy) {
            file_entropy.push(v);
        }
        if let Some(v) = record.facts.num(FactId::OverlayHighEntropy) {
            overlay_entropy.push(v);
        }
        if let Some(v) = record.facts.num(FactId::ImportsSparse) {
            imports.push(v);
        }
        for (id, cell) in record.facts.iter() {
            if cell.val == Tri::True {
                match fired.iter_mut().find(|(f, _)| *f == id) {
                    Some((_, n)) => *n += 1,
                    None => fired.push((id, 1)),
                }
            }
            if cell.val == Tri::Unknown {
                match unknown.iter_mut().find(|(f, _)| *f == id) {
                    Some((_, n)) => *n += 1,
                    None => unknown.push((id, 1)),
                }
            }
        }
    }

    println!("标定：干净语料上的分位数（用于确定判定带，而不是拍脑袋取阈值）");
    show_quantiles("最高节区熵", &mut section_entropy);
    show_quantiles("整文件熵", &mut file_entropy);
    show_quantiles("叠加段熵", &mut overlay_entropy);
    show_quantiles("导入函数数", &mut imports);
    println!(
        "  当前判定带：节区熵 ({:.2}, {:.2}) · 文件熵 ({:.2}, {:.2}) · 叠加段熵 ({:.2}, {:.2})",
        t3_format::calibration::SECTION_ENTROPY.lo,
        t3_format::calibration::SECTION_ENTROPY.hi,
        t3_format::calibration::FILE_ENTROPY.lo,
        t3_format::calibration::FILE_ENTROPY.hi,
        t3_format::calibration::OVERLAY_ENTROPY.lo,
        t3_format::calibration::OVERLAY_ENTROPY.hi
    );
    println!();

    let n = records.iter().filter(|r| r.kind == "pe").count().max(1);
    println!("（以下比例的分母是 {} 个 PE 文件）", n);
    fired.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    println!("事实在干净语料上的命中率（越高越说明该事实没有区分度）");
    for (id, count) in fired.iter().take(12) {
        println!(
            "  {:<30}{:>6}  ({:.2}%)",
            id.name(),
            count,
            *count as f64 * 100.0 / n as f64
        );
    }
    unknown.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    if !unknown.is_empty() {
        println!("未决率最高的若干事实");
        for (id, count) in unknown.iter().take(6) {
            println!(
                "  {:<30}{:>6}  ({:.2}%)",
                id.name(),
                count,
                *count as f64 * 100.0 / n as f64
            );
        }
    }
    println!();

    let mut clean = 0usize;
    let mut suspicious = 0usize;
    let mut malicious = 0usize;
    let mut undecided = 0usize;
    let mut conflict = 0usize;
    for record in &records {
        match record.verdict.exit_code() {
            0 => clean += 1,
            1 => suspicious += 1,
            2 => malicious += 1,
            3 => undecided += 1,
            4 => conflict += 1,
            _ => {}
        }
    }
    println!("在同一干净语料上的判定分布");
    println!(
        "  干净 {clean} ({:.2}%) · 可疑 {suspicious} ({:.2}%) · 恶意 {malicious} ({:.2}%) · 未定 {undecided} ({:.2}%) · 冲突 {conflict}",
        clean as f64 * 100.0 / n as f64,
        suspicious as f64 * 100.0 / n as f64,
        malicious as f64 * 100.0 / n as f64,
        undecided as f64 * 100.0 / n as f64
    );
    println!("  这组数字是误报率的直接测量：干净语料上「恶意」的比例就是当前规则的假阳率下界。");
}

fn show_quantiles(name: &str, values: &mut [f32]) {
    if values.is_empty() {
        println!("  {name:<12} 无样本");
        return;
    }
    let mut sorted = values.to_vec();
    println!(
        "  {name:<12} n={:<6} p50={:.3} p95={:.3} p99={:.3} p99.5={:.3} p99.9={:.3} max={:.3}",
        sorted.len(),
        quantile_of(&mut sorted, 0.50),
        quantile_of(&mut sorted, 0.95),
        quantile_of(&mut sorted, 0.99),
        quantile_of(&mut sorted, 0.995),
        quantile_of(&mut sorted, 0.999),
        quantile_of(&mut sorted, 1.0)
    );
}

fn measure<F, T>(f: F) -> f64
where
    F: Fn() -> T,
{
    // One warm-up pass, then the best of five: the minimum is the least noisy
    // estimator when the interference is additive.
    //
    // The result goes through `black_box`. Without it the optimiser is entitled to
    // delete work whose result is discarded, and the first version of this benchmark
    // did exactly that: it reported a byte-count loop at 104 TiB/s, which is the speed
    // of not running it. A benchmark measured against the optimiser measures nothing.
    std::hint::black_box(f());
    let mut best = f64::INFINITY;
    for _ in 0..5 {
        let started = Instant::now();
        std::hint::black_box(f());
        let secs = started.elapsed().as_secs_f64();
        if secs > 0.0 {
            best = best.min(secs);
        }
    }
    BUFFER as f64 / best / (1024.0 * 1024.0 * 1024.0)
}

/// Folds every bin into one number, so the optimiser cannot specialise away any part of
/// the histogram.
fn checksum(histogram: &[u32; 256]) -> u64 {
    histogram.iter().map(|&bin| u64::from(bin)).sum()
}

fn threads() -> usize {
    std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(0)
}

fn avx2_available() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        is_x86_feature_detected!("avx2")
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

/// Buffer with long runs of one value, which is what real binaries look like:
/// alignment padding, zero-filled tails, repeated opcodes.
fn skewed_buffer(len: usize, run: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    let mut s = 0x1234_5678u32;
    let mut i = 0;
    while i < len {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        let value = (s >> 24) as u8;
        let end = (i + run).min(len);
        for slot in &mut out[i..end] {
            *slot = value;
        }
        i = end;
    }
    out
}

/// Uniform random bytes: the distribution of a packed or encrypted section, and the
/// case where a single histogram table never stalls.
fn noise_buffer(len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    let mut s = 0x9E37_79B9_7F4A_7C15u64;
    for slot in out.iter_mut() {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        *slot = (s >> 40) as u8;
    }
    out
}
