//! 三值算子库基准。
//!
//! 默认跑 SmolLM2-135M 的 decode 真实形状，折算成端到端 tok/s 代理值；
//! 也可以用 `--rows/--cols` 只跑单个形状。

use trit_ops::bench::{format_table, measure};
use trit_ops::pool;

fn arg_usize(args: &[String], name: &str) -> Option<usize> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<usize>().ok())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let threads = arg_usize(&args, "--threads").unwrap_or(8);
    pool::set_threads(threads);
    println!("线程 = {threads}，AVX2 = {}", trit_ops::simd::avx2());

    if args.iter().any(|a| a == "--stream") {
        // 1.34 亿参数一次性过一遍：工作集远大于缓存，测的是内存墙
        let rows = 131_072usize;
        let cols = 1024usize;
        let iters = arg_usize(&args, "--iters").unwrap_or(3);
        println!(
            "
流式模式：{rows}×{cols} = {} M 参数，每次 {iters} 轮",
            rows * cols / 1_000_000
        );
        let res = measure(rows, cols, iters);
        print!("{}", format_table(&res));
        return;
    }

    if let (Some(rows), Some(cols)) = (arg_usize(&args, "--rows"), arg_usize(&args, "--cols")) {
        let iters = arg_usize(&args, "--iters").unwrap_or(200);
        let res = measure(rows, cols, iters);
        println!("\n形状 {rows}×{cols}，{iters} 次/内核");
        print!("{}", format_table(&res));
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

    let mut per_shape = Vec::new();
    for (name, rows, cols, count) in shapes {
        let macs = (rows * cols) as f64;
        let iters = ((2e8 / macs) as usize).clamp(5, 2000);
        println!("\n=== {name}  {rows}×{cols}  （每 token {count} 次，{iters} 次测量）");
        let res = measure(rows, cols, iters);
        print!("{}", format_table(&res));
        per_shape.push((count, res));
    }

    // 汇总：把每个形状的耗时按每 token 次数加权
    println!("\n=== 端到端折算（SmolLM2-135M decode，134.5M MAC/token）");
    let kernels = per_shape[0].1.len();
    println!(
        "{:<26} {:>12} {:>12} {:>10}",
        "内核", "ms/token", "tok/s", "相对 f32"
    );
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
        println!(
            "{:<26} {:>12.2} {:>12.1} {:>9.2}x",
            name,
            ns / 1e6,
            tok,
            tok / base_tok
        );
    }
}
