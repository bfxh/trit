//! 评测数据读取（从 run.rs 拆出，守住行数预算）。
use trit_wspace::activations;
pub(crate) struct Corpus {
    pub(crate) batch: usize,
    pub(crate) seq: usize,
    pub(crate) hidden: usize,
    pub(crate) mask: Vec<u8>,
    pub(crate) input: Vec<f32>,
    pub(crate) reference: Vec<f32>,
}
/// 逐层输出只用于对齐诊断；全量下 6 层要 1.6 GB，默认关。
pub(crate) fn keep_layers() -> bool {
    std::env::var_os("TRIT_NO_LAYERS").is_none()
}
pub(crate) fn load(dir: &str) -> Corpus {
    // 全量 STS 67.8M 元素 > 默认 MAX_ELEMENTS（16.7M）；SciFact 154.6M 元素，需更大上限。
    let big = 1usize << 29;
    let (batch, seq, mask) = activations::read_mask(format!("{dir}/mask.u8")).expect("mask");
    let (input_rows, hidden, input) =
        activations::read_matrix_with_limit(format!("{dir}/input.f32"), big).expect("input");
    let (ref_rows, ref_cols, reference) =
        activations::read_matrix_with_limit(format!("{dir}/reference.f32"), big)
            .expect("reference");
    assert!(input_rows == batch * seq && ref_rows == batch * seq && ref_cols == hidden);
    Corpus {
        batch,
        seq,
        hidden,
        mask,
        input,
        reference,
    }
}
