//! bert6 的命令行参数（从 `run/args.rs` 拆出守住行数预算）。

pub(crate) struct Args {
    pub(crate) file: String,
    pub(crate) data_dir: String,
    pub(crate) out: Option<String>,
    pub(crate) seed: u64,
    pub(crate) activation: String,
    pub(crate) gptq: Option<usize>,
    pub(crate) sidecar_svd: Option<Vec<usize>>,
}

/// 逗号分隔的 rank 列表（`0,16,64`）；去重并升序，读数按同一顺序输出。
fn ranks(value: &str) -> Vec<usize> {
    let mut out: Vec<usize> = value
        .split(',')
        .filter(|item| !item.trim().is_empty())
        .map(|item| item.trim().parse().expect("rank"))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

pub(crate) fn parse() -> Args {
    let (mut file, mut data_dir) = (String::new(), String::new());
    let mut out = None;
    let mut seed = 7u64;
    let mut activation = "all".to_string();
    let mut gptq = None;
    let mut sidecar_svd = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--file" => file = args.next().expect("file"),
            "--data-dir" => data_dir = args.next().expect("data-dir"),
            "--out" => out = Some(args.next().expect("out")),
            "--seed" => seed = args.next().expect("seed").parse().expect("num"),
            "--activation-mode" => activation = args.next().expect("activation-mode"),
            "--gptq" => gptq = Some(0),
            "--gptq-group" => gptq = Some(args.next().expect("group").parse().expect("num")),
            "--sidecar-svd" => sidecar_svd = Some(ranks(&args.next().expect("sidecar-svd"))),
            other => panic!("unknown arg {other}"),
        }
    }
    assert!(
        !file.is_empty() && !data_dir.is_empty(),
        "--file/--data-dir 必填"
    );
    Args {
        file,
        data_dir,
        out,
        seed,
        activation,
        gptq,
        sidecar_svd,
    }
}
