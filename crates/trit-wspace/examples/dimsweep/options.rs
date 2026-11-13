//! dimsweep 的命令行参数（从 `main.rs` 拆出）。
use crate::{EMBEDDINGS, QUERY, parse_list};
pub struct Args {
    pub safetensors: String,
    pub query: String,
    pub embeddings: String,
    pub readout_row: usize,
    pub samples: usize,
    pub edit_limit: usize,
    pub keep_limit: usize,
    pub seed: u64,
    pub pca_samples: usize,
    pub max_k: usize,
    pub iterations: usize,
    pub basis: String,
    pub teacher: String,
    pub criterion: String,
    pub dims: Vec<usize>,
    pub steps_dims: Vec<usize>,
    pub max_edits: Vec<usize>,
    pub beam_widths: Vec<usize>,
    pub out: Option<String>,
}

pub fn parse_args() -> Args {
    let mut args = Args {
        safetensors: String::new(),
        query: QUERY.to_string(),
        embeddings: EMBEDDINGS.to_string(),
        readout_row: 0,
        samples: 512,
        edit_limit: 8,
        keep_limit: 64,
        seed: 7,
        pca_samples: 2048,
        max_k: 128,
        iterations: 12,
        basis: "pca".to_string(),
        teacher: "linear".to_string(),
        criterion: "teacher".to_string(),
        dims: vec![384, 128, 64, 32, 16],
        steps_dims: vec![32, 16],
        max_edits: vec![1, 2],
        beam_widths: vec![8],
        out: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut next = || it.next().expect("缺少参数值");
        match arg.as_str() {
            "--safetensors" => args.safetensors = next(),
            "--query" => args.query = next(),
            "--embeddings" => args.embeddings = next(),
            "--readout-row" => args.readout_row = next().parse().expect("readout-row"),
            "--samples" => args.samples = next().parse().expect("samples"),
            "--edit-limit" => args.edit_limit = next().parse().expect("edit-limit"),
            "--keep-limit" => args.keep_limit = next().parse().expect("keep-limit"),
            "--seed" => args.seed = next().parse().expect("seed"),
            "--pca-samples" => args.pca_samples = next().parse().expect("pca-samples"),
            "--max-k" => args.max_k = next().parse().expect("max-k"),
            "--iterations" => args.iterations = next().parse().expect("iterations"),
            "--basis" => args.basis = next(),
            "--teacher" => args.teacher = next(),
            "--criterion" => args.criterion = next(),
            "--dims" => args.dims = parse_list(&next()),
            "--steps-dims" => args.steps_dims = parse_list(&next()),
            "--max-edits" => args.max_edits = parse_list(&next()),
            "--beam-widths" => args.beam_widths = parse_list(&next()),
            "--out" => args.out = Some(next()),
            other => panic!("未知参数 {other}"),
        }
    }
    assert!(!args.safetensors.is_empty(), "需要 --safetensors");
    assert!(args.max_k > 0, "--max-k 必须 > 0");
    args
}
