pub(crate) struct Args {
    pub(crate) file: String,
    pub(crate) data_dir: String,
    pub(crate) out: Option<String>,
    pub(crate) seed: u64,
    pub(crate) activation: String,
    pub(crate) gptq: Option<usize>,
}

pub(crate) fn parse() -> Args {
    let (mut file, mut data_dir) = (String::new(), String::new());
    let mut out = None;
    let mut seed = 7u64;
    let mut activation = "all".to_string();
    let mut gptq = None;
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
    }
}
