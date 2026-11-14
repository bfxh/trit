//! dimsweep 的扫描上下文：三条曲线共用的一次性参数。
//!
//! 从 `curve.rs` 拆出：module_budget 台账只收紧、不抬高单文件行数。

use crate::basis::Sample;
use crate::judge::Teacher;
use crate::options::Args;
use trit_wspace::Net;
use trit_wspace::safetensors::Weights;

pub struct CurveCtx<'a> {
    pub args: &'a Args,
    pub query: &'a Weights,
    pub readout: &'a [f32],
    pub second: Option<&'a [f32]>,
    pub kind: Teacher,
    pub dim: usize,
    pub comps: &'a [Vec<f64>],
    pub student: &'a Net,
    pub samples: &'a [Sample],
    pub edit: &'a [Sample],
    pub keep: &'a [Sample],
    pub criterion_full: bool,
}
