//! dimsweep 的命令行参数（结构体）。

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
    pub out_basis: Option<String>,
}
