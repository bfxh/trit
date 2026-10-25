//! 三值算子库基准。
//!
//! 默认跑 SmolLM2-135M 的 decode 真实形状，折算成端到端 tok/s 代理值；
//! 也可以用 `--rows/--cols` 只跑单个形状。
//!
//! 长跑不沉默：`--rounds` 每轮结束立即 flush 一行进展，`--split` 分两段报。
//!
//! ```text
//! bench                                   # 默认形状集
//! bench --stream --iters 3                # 流式（内存墙）
//! bench --rows 576 --cols 576             # 单形状
//! bench --rows 576 --cols 576 --rounds 5  # 轮转测量，逐轮打印
//! bench --rows 576 --cols 576 --rounds 5 --pair 掩码 tryte
//! bench --rows 576 --cols 576 --split     # tryte 建表 vs 查表
//! bench --rows 1024 --cols 576 --split-map --rounds 7   # 拆表查表：%9//9 vs 映射
//! ```

use std::io::Write;

use trit_ops::bench::{
    Measurement, format_table, measure, measure_rounds, round_spread, tryte_split,
    tryte_split_lookup,
};
use trit_ops::pool;

fn arg_usize(args: &[String], name: &str) -> Option<usize> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<usize>().ok())
}

/// 取 `--pair A B` 的两个值。
fn arg_pair<'a>(args: &'a [String], name: &str) -> Option<(&'a str, &'a str)> {
    let i = args.iter().position(|a| a == name)?;
    Some((args.get(i + 1)?.as_str(), args.get(i + 2)?.as_str()))
}

/// 立即刷出，保证长跑时能看到进展。
fn emit(line: &str) {
    println!("{line}");
    let _ = std::io::stdout().flush();
}

/// 轮转测量 + 逐轮打印，返回 (中位数表, 每轮原始数据)。
fn run_rounds(
    rows: usize,
    cols: usize,
    iters: usize,
    rounds: usize,
    title: &str,
) -> (Vec<Measurement>, Vec<Vec<Measurement>>) {
    emit(&format!(
        "\n{title}  {rows}×{cols}  {rounds} 轮 × {iters} 次/内核"
    ));
    let mut history: Vec<Vec<Measurement>> = Vec::with_capacity(rounds);
    let med = measure_rounds(rows, cols, iters, rounds, |r, ms| {
        // 每轮一行：内核缩写 + GMAC/s
        let mut line = format!("  [轮 {}/{}]", r + 1, rounds);
        for m in ms {
            line.push_str(&format!("  {} {:.1}", short_name(&m.name), m.gmac_s));
        }
        emit(&line);
        history.push(ms.to_vec());
    });
    emit("");
    (med, history)
}

/// 内核名缩写，只为逐轮那行不至于太长。
fn short_name(name: &str) -> &str {
    const KEYS: [(&str, &str); 7] = [
        ("f32", "f32"),
        ("f16", "f16"),
        ("Q4_0", "q4"),
        ("标量", "scalar"),
        ("掩码", "mask"),
        ("tryte", "tryte"),
        ("自动", "auto"),
    ];
    for (k, v) in KEYS {
        if name.contains(k) {
            return v;
        }
    }
    name
}

/// 打印中位数表，并在有轮次数据时附上波动区间。
fn print_median_table(med: &[Measurement], history: &[Vec<Measurement>]) {
    print!("{}", format_table(med));
    if history.len() > 1 {
        emit(&format!("（{} 轮中位数；各轮波动：）", history.len()));
        for (k, m) in med.iter().enumerate() {
            let (lo, hi) = round_spread(history, k);
            emit(&format!(
                "  {:<26} {:.1} … {:.1} GMAC/s（±{:.0}%）",
                m.name,
                lo,
                hi,
                (hi / lo - 1.0) * 100.0
            ));
        }
    }
}

/// 两个内核的逐轮配对比较 + 符号检验。
fn print_pair(med: &[Measurement], history: &[Vec<Measurement>], a: &str, b: &str) {
    let find = |s: &str| med.iter().position(|m| m.name.contains(s));
    let (Some(ia), Some(ib)) = (find(a), find(b)) else {
        emit(&format!("找不到内核：{a} / {b}"));
        return;
    };
    emit(&format!(
        "\n配对比较：{}  vs  {}（每轮交替测量）",
        med[ia].name, med[ib].name
    ));
    emit(&format!(
        "{:<6} {:>12} {:>12} {:>10}",
        "轮次", "A GMAC/s", "B GMAC/s", "B/A"
    ));
    let mut b_wins = 0usize;
    for (i, r) in history.iter().enumerate() {
        let (av, bv) = (r[ia].gmac_s, r[ib].gmac_s);
        if bv > av {
            b_wins += 1;
        }
        emit(&format!(
            "{:<6} {:>12.2} {:>12.2} {:>9.3}x",
            i + 1,
            av,
            bv,
            bv / av
        ));
    }
    emit(&format!(
        "{:<6} {:>12.2} {:>12.2} {:>9.3}x",
        "中位",
        med[ia].gmac_s,
        med[ib].gmac_s,
        med[ib].gmac_s / med[ia].gmac_s
    ));
    emit(&format!(
        "符号检验：B 更快的轮数 {b_wins}/{}",
        history.len()
    ));
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let threads = arg_usize(&args, "--threads").unwrap_or(8);
    pool::set_threads(threads);
    emit(&format!(
        "线程 = {threads}，AVX2 = {}",
        trit_ops::simd::avx2()
    ));

    // tryte 建表 vs 查表分解
    if args.iter().any(|a| a == "--split") {
        let (rows, cols) = (
            arg_usize(&args, "--rows").unwrap_or(576),
            arg_usize(&args, "--cols").unwrap_or(576),
        );
        let iters = arg_usize(&args, "--iters").unwrap_or(200);
        let c = tryte_split(rows, cols, iters);
        let whole = c.build_whole + c.lookup_whole;
        let split = c.build_split + c.lookup_split;
        let split_map = c.build_split + c.lookup_split_map;
        emit(&format!("\ntryte 成本分解  {rows}×{cols}  {iters} 次/测"));
        emit(&format!(
            "{:<20}{:>12}{:>10}{:>12}{:>12}{:>10}",
            "表粒度", "建表 ns", "建表占比", "查表 ns", "合计 ns", "相对整表"
        ));
        emit(&format!(
            "{:<20}{:>12.1}{:>9.1}%{:>12.1}{:>12.1}{:>9.3}x",
            "整字节 243 项",
            c.build_whole,
            c.build_whole / whole * 100.0,
            c.lookup_whole,
            whole,
            1.0
        ));
        emit(&format!(
            "{:<20}{:>12.1}{:>9.1}%{:>12.1}{:>12.1}{:>9.3}x",
            "拆表 2+3 = 36 项",
            c.build_split,
            c.build_split / split * 100.0,
            c.lookup_split,
            split,
            split / whole
        ));
        emit(&format!(
            "{:<20}{:>12.1}{:>9.1}%{:>12.1}{:>12.1}{:>9.3}x",
            "拆表 + 映射查表",
            c.build_split,
            c.build_split / split_map * 100.0,
            c.lookup_split_map,
            split_map,
            split_map / whole
        ));
        return;
    }

    // 拆表查表：%9//9 vs 预计算映射
    if args.iter().any(|a| a == "--split-map") {
        let (rows, cols) = (
            arg_usize(&args, "--rows").unwrap_or(1024),
            arg_usize(&args, "--cols").unwrap_or(576),
        );
        let iters = arg_usize(&args, "--iters").unwrap_or(200);
        let rounds = arg_usize(&args, "--rounds").unwrap_or(7);
        let (divmod, map) = tryte_split_lookup(rows, cols, iters, rounds);
        emit(&format!(
            "\n拆表查表  {rows}×{cols}  {rounds} 轮 × {iters} 次（单次 gemv）"
        ));
        emit(&format!(
            "{:<6}{:>14}{:>14}{:>12}",
            "轮次", "除法 ns", "映射 ns", "映射/除法"
        ));
        let mut wins = 0usize;
        for i in 0..rounds {
            if map[i] < divmod[i] {
                wins += 1;
            }
            emit(&format!(
                "{:<6}{:>14.1}{:>14.1}{:>11.3}x",
                i + 1,
                divmod[i],
                map[i],
                map[i] / divmod[i]
            ));
        }
        let med = |v: &[f64]| {
            let mut s = v.to_vec();
            s.sort_by(|a, b| a.partial_cmp(b).expect("有限值"));
            s[s.len() / 2]
        };
        let (a, b) = (med(&divmod), med(&map));
        emit(&format!(
            "{:<6}{:>14.1}{:>14.1}{:>11.3}x",
            "中位",
            a,
            b,
            b / a
        ));
        emit(&format!("符号检验：映射更快的轮数 {wins}/{rounds}"));
        return;
    }

    if args.iter().any(|a| a == "--stream") {
        // 1.34 亿参数一次性过一遍：工作集远大于缓存，测的是内存墙
        let rows = 131_072usize;
        let cols = 1024usize;
        let iters = arg_usize(&args, "--iters").unwrap_or(3);
        emit(&format!(
            "\n流式模式：{rows}×{cols} = {} M 参数，每次 {iters} 轮",
            rows * cols / 1_000_000
        ));
        let res = measure(rows, cols, iters);
        print!("{}", format_table(&res));
        return;
    }

    if let (Some(rows), Some(cols)) = (arg_usize(&args, "--rows"), arg_usize(&args, "--cols")) {
        let iters = arg_usize(&args, "--iters").unwrap_or(200);
        let rounds = arg_usize(&args, "--rounds").unwrap_or(1);
        if rounds > 1 {
            let (med, history) = run_rounds(rows, cols, iters, rounds, "轮转测量");
            print_median_table(&med, &history);
            if let Some((a, b)) = arg_pair(&args, "--pair") {
                print_pair(&med, &history, a, b);
            }
        } else {
            emit(&format!("\n形状 {rows}×{cols}，{iters} 次/内核"));
            let res = measure(rows, cols, iters);
            print!("{}", format_table(&res));
        }
        return;
    }

    // SmolLM2-135M：576 维、ff=1536、9/3 头、vocab=49152、30 层
    let shapes: [(&str, usize, usize, usize); 5] = [
        ("q/o 投影", 576, 576, 60),
        ("k/v 投影(GQA)", 192, 576, 60),
        ("gate/up", 1536, 576, 60),
        ("down", 576, 1536, 30),
        ("输出层", 49152, 576, 1),
    ];
    let rounds = arg_usize(&args, "--rounds").unwrap_or(1);

    let mut per_shape = Vec::new();
    for (name, rows, cols, count) in shapes {
        let macs = (rows * cols) as f64;
        let iters = ((2e8 / macs) as usize).clamp(5, 2000);
        if rounds > 1 {
            let (med, history) = run_rounds(rows, cols, iters, rounds, name);
            print_median_table(&med, &history);
            per_shape.push((count, med));
        } else {
            emit(&format!(
                "\n=== {name}  {rows}×{cols}  （每 token {count} 次，{iters} 次测量）"
            ));
            let res = measure(rows, cols, iters);
            print!("{}", format_table(&res));
            per_shape.push((count, res));
        }
    }

    // 汇总：把每个形状的耗时按每 token 次数加权
    emit("\n=== 端到端折算（SmolLM2-135M decode，134.5M MAC/token）");
    let kernels = per_shape[0].1.len();
    emit(&format!(
        "{:<26} {:>12} {:>12} {:>10}",
        "内核", "ms/token", "tok/s", "相对 f32"
    ));
    let mut base_tok = 0.0f64;
    for k in 0..kernels {
        let name = per_shape[0].1[k].name.clone();
        let mut ns = 0.0f64;
        for (count, res) in &per_shape {
            ns += *count as f64 * res[k].ns;
        }
        let tok = 1e9 / ns;
        if k == 0 {
            base_tok = tok;
        }
        emit(&format!(
            "{:<26} {:>12.2} {:>12.1} {:>9.2}x",
            name,
            ns / 1e6,
            tok,
            tok / base_tok
        ));
    }
}
