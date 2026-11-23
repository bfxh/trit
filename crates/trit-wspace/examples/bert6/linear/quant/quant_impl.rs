//! 三值线性层：absmean / GPTQ，per-row 或 per-group scale + f32 bias；可选列侧车或 SVD 侧车。
//! 从 `linear/quant.rs` 拆出，原文件只留两行接线（守住模块预算）。

use crate::linear::F32Linear;
use crate::sidecar_layer::Sidecar;
use trit_wspace::residual::LowRankResidual;

pub(crate) struct QuantLinear {
    rows: usize,
    cols: usize,
    group: usize,
    groups: usize,
    trits: Vec<i8>,
    scales: Vec<f32>,
    bias: Vec<f32>,
    sidecar: Option<Sidecar>,
    residual: Option<LowRankResidual>,
}

impl QuantLinear {
    /// `group == 0` 或 `group >= cols` 时退化为 per-row scale（每行一个）。
    /// 列侧车只在 per-row 时使用（`run.rs` 的侧车扫描不开 GPTQ）；SVD 侧车
    /// 与 GPTQ/per-group 叠加，残差按实际生效的块 scale 计算。
    pub(crate) fn from(
        linear: &F32Linear,
        cols_rank: usize,
        svd_rank: usize,
        h: Option<&[f32]>,
        group: usize,
    ) -> Self {
        let group = if group == 0 {
            linear.cols
        } else {
            group.clamp(1, linear.cols)
        };
        let (trits, scales) = match h {
            Some(h) if h.len() == linear.cols * linear.cols => trit_wspace::gptq::quantize_grouped(
                &linear.weight,
                linear.rows,
                linear.cols,
                h,
                group,
            )
            .expect("GPTQ 三值化失败"),
            _ => trit_wspace::gptq::absmean(&linear.weight, linear.rows, linear.cols)
                .expect("absmean 三值化失败"),
        };
        let sidecar = Sidecar::build(linear, &trits, &scales, cols_rank);
        let residual = LowRankResidual::build(
            &linear.weight,
            linear.rows,
            linear.cols,
            &trits,
            &scales,
            if h.is_some() { group } else { linear.cols },
            svd_rank,
        )
        .expect("SVD 侧车失败");
        Self {
            rows: linear.rows,
            cols: linear.cols,
            group,
            groups: linear.cols.div_ceil(group),
            trits,
            scales,
            bias: linear.bias.clone(),
            sidecar,
            residual,
        }
    }

    fn forward(&self, input: &[f32], out: &mut [f32]) {
        let projections = self
            .residual
            .as_ref()
            .map(|residual| residual.projections(input));
        for (row, slot) in out.iter_mut().enumerate().take(self.rows) {
            let mut total = 0.0f64;
            for chunk in 0..self.groups {
                let start = chunk * self.group;
                let end = (start + self.group).min(self.cols);
                let line = &self.trits[row * self.cols + start..row * self.cols + end];
                let mut acc = 0.0f64;
                for (&trit, &value) in line.iter().zip(&input[start..end]) {
                    acc += f64::from(trit) * f64::from(value);
                }
                total += f64::from(self.scales[row * self.groups + chunk]) * acc;
            }
            let correction = self
                .sidecar
                .as_ref()
                .map_or(0.0, |sidecar| sidecar.correction(row, input));
            let residual = match (&self.residual, &projections) {
                (Some(residual), Some(projections)) => residual.correction(row, projections),
                _ => 0.0,
            };
            *slot = (total + f64::from(self.bias[row]) + correction + residual) as f32;
        }
    }

    pub(crate) fn forward_seq(&self, input: &[f32], seq: usize, out: &mut [f32]) {
        for token in 0..seq {
            self.forward(
                &input[token * self.cols..(token + 1) * self.cols],
                &mut out[token * self.rows..(token + 1) * self.rows],
            );
        }
    }

    /// 只收缩 SVD 侧车的生效 rank；列侧车不动。
    pub(crate) fn set_sidecar_rank(&mut self, rank: usize) {
        if let Some(residual) = &mut self.residual {
            residual.set_rank(rank).expect("SVD rank 收缩");
        }
    }

    /// 侧车占用的 f32 个数（列侧车 + SVD 侧车）。
    pub(crate) fn sidecar_params(&self) -> usize {
        self.sidecar.as_ref().map_or(0, |sidecar| sidecar.params())
            + self
                .residual
                .as_ref()
                .map_or(0, |residual| residual.params())
    }

    /// 该层 f32 权重个数（三值主干同位置）。
    pub(crate) fn weight_params(&self) -> usize {
        self.rows * self.cols
    }
}
