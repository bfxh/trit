//! 三值 3D 资产神经组件（M0）：体素编码器 + 几何头。
//!
//! - [`AttentionEncoder`]：把 `grid³` 体素切成 `patch³` token，走
//!   [`trit_attn::AttentionBlock`]（三值 QKV/FFN、全链 STE），mean-pool 成 latent。
//! - [`DenseEncoder`]：`TernaryDense` 堆叠的对照基线。
//! - [`VoxelHead`]：latent → 体素值（tanh），供占据/几何监督。
//!
//! 三值位置（`POLICY.md` §1 四类账）：
//! - **A 原生红利**：QKV / FFN / 全连接权重三值（0.2 B/参数，乘法变加减），
//!   训练与推理同一套三值前向（QAT），没有“导出再量化”的落差。
//! - **B 模拟税**：latent、体素输入/输出与坐标是连续量，必须 f32；
//!   三值化会直接毁掉几何。
//! - **C 生态路径依赖**：输入/输出暂用 f32 体素 + OBJ/SurfaceNets 生态。
//! - **D 基数无关**：注意力/矩阵乘复杂度与进制无关。
//!
//! M0 边界：单样本/小网格；没有位置编码、LayerNorm、多尺度；几何头只做体素回归，
//! 高斯/UV/材质/光照头是后续步骤。

pub mod gaussian;
pub mod lut;
pub mod mesh;

use trit_attn::{AttentionBlock, BlockOptimizers};
use trit_learn::{Activation, Adam, Ste, TernaryDense};

/// 三值体素网格：`-1` / `0` / `+1` 行主序（`x` 最快，然后 `y`、`z`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoxelGrid {
    /// 每边体素数。
    pub size: usize,
    /// `size³` 个三值体素。
    pub cells: Vec<i8>,
}

impl VoxelGrid {
    /// 全 `-1`（外部）网格。
    pub fn new(size: usize) -> Self {
        assert!(size > 0, "size 必须非零");
        Self {
            size,
            cells: vec![-1; size * size * size],
        }
    }

    /// 内部填充 `+1` 的实心立方体（留 `margin` 格外壳）。
    pub fn cube(size: usize, margin: usize) -> Self {
        let mut g = Self::new(size);
        for z in margin..size.saturating_sub(margin) {
            for y in margin..size.saturating_sub(margin) {
                for x in margin..size.saturating_sub(margin) {
                    g.set(x, y, z, 1);
                }
            }
        }
        g
    }

    /// 索引。
    pub fn idx(&self, x: usize, y: usize, z: usize) -> usize {
        assert!(x < self.size && y < self.size && z < self.size, "体素越界");
        (z * self.size + y) * self.size + x
    }

    /// 读值。
    pub fn get(&self, x: usize, y: usize, z: usize) -> i8 {
        self.cells[self.idx(x, y, z)]
    }

    /// 写值（只接受 -1/0/+1）。
    pub fn set(&mut self, x: usize, y: usize, z: usize, v: i8) {
        assert!(matches!(v, -1..=1), "体素必须是 -1/0/+1");
        let i = self.idx(x, y, z);
        self.cells[i] = v;
    }

    /// 扁平成 f32（保持 -1/0/+1，不做归一化）。
    pub fn flatten_f32(&self) -> Vec<f32> {
        self.cells.iter().map(|&v| f32::from(v)).collect()
    }
}

/// 确定性初始化：随机符号、幅值 `U(0.1, 0.3)`（与 `AttentionBlock` 同源约定）。
pub(crate) fn xorshift_init(seed: u64) -> impl FnMut(usize, usize) -> f32 {
    let mut state = seed | 1;
    move |_, _| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let magnitude = 0.1 + ((state % 200) as f32) / 1000.0;
        if state & 1 == 0 {
            magnitude
        } else {
            -magnitude
        }
    }
}

/// 把体素网格切成 `patch³` token；返回 `(token 数, token 维)`。
fn patch_geom(grid: usize, patch: usize) -> (usize, usize) {
    assert!(
        patch > 0 && grid > 0 && grid.is_multiple_of(patch),
        "grid 必须被 patch 整除"
    );
    let per_side = grid / patch;
    (per_side * per_side * per_side, patch * patch * patch)
}

/// 体素 → token 序列（`T×dim` 行主序，值为 `体素 × input_scale`）。
fn tokenize(g: &VoxelGrid, patch: usize, dim: usize, input_scale: f32) -> Vec<f32> {
    let per_side = g.size / patch;
    let mut seq = Vec::with_capacity(per_side * per_side * per_side * dim);
    for tz in 0..per_side {
        for ty in 0..per_side {
            for tx in 0..per_side {
                for dz in 0..patch {
                    for dy in 0..patch {
                        for dx in 0..patch {
                            seq.push(
                                f32::from(g.get(tx * patch + dx, ty * patch + dy, tz * patch + dz))
                                    * input_scale,
                            );
                        }
                    }
                }
            }
        }
    }
    seq
}

/// 三值注意力体素编码器。
pub struct AttentionEncoder {
    block: AttentionBlock,
    grid: usize,
    patch: usize,
    dim: usize,
    tokens: usize,
    input_scale: f32,
}

impl AttentionEncoder {
    /// `grid` 每边体素数，`patch` 每边 token 覆盖的体素数，`hidden` FFN 宽度。
    pub fn new(grid: usize, patch: usize, hidden: usize, seed: u64) -> Self {
        let (tokens, dim) = patch_geom(grid, patch);
        Self {
            block: AttentionBlock::new(dim, hidden, Activation::Tanh, Ste::Identity, seed),
            grid,
            patch,
            dim,
            tokens,
            input_scale: 1.0,
        }
    }

    /// latent 维数（= token 维）。
    pub fn latent_dim(&self) -> usize {
        self.dim
    }

    /// token 数。
    pub fn tokens(&self) -> usize {
        self.tokens
    }

    /// 展平 token 序列的维数（`tokens × dim`）。
    pub fn flat_dim(&self) -> usize {
        self.tokens * self.dim
    }

    /// 编码为完整 token 序列（`T×dim`，保留空间信息）。
    pub fn encode_flat(&mut self, g: &VoxelGrid) -> Vec<f32> {
        assert_eq!(g.size, self.grid, "网格尺寸与编码器不一致");
        let seq = tokenize(g, self.patch, self.dim, self.input_scale);
        self.block.forward(&seq)
    }

    /// 完整 token 序列的反向。
    pub fn backward_flat(&mut self, grad_out: &[f32]) -> Vec<f32> {
        assert_eq!(grad_out.len(), self.flat_dim(), "grad_out 维度不对");
        self.block.backward(grad_out)
    }

    /// 编码：返回 mean-pool 后的 `dim` 维 latent。
    pub fn encode(&mut self, g: &VoxelGrid) -> Vec<f32> {
        assert_eq!(g.size, self.grid, "网格尺寸与编码器不一致");
        let seq = tokenize(g, self.patch, self.dim, self.input_scale);
        let out = self.block.forward(&seq);
        let inv = 1.0 / self.tokens as f32;
        let mut latent = vec![0.0f32; self.dim];
        for t in 0..self.tokens {
            for d in 0..self.dim {
                latent[d] += out[t * self.dim + d] * inv;
            }
        }
        latent
    }

    /// 反向：`grad_latent` 经 mean-pool 广播回 token，返回对 token 输入的梯度。
    pub fn backward(&mut self, grad_latent: &[f32]) -> Vec<f32> {
        assert_eq!(grad_latent.len(), self.dim, "grad_latent 维度不对");
        let inv = 1.0 / self.tokens as f32;
        let mut grad_tokens = vec![0.0f32; self.tokens * self.dim];
        for t in 0..self.tokens {
            for d in 0..self.dim {
                grad_tokens[t * self.dim + d] = grad_latent[d] * inv;
            }
        }
        self.block.backward(&grad_tokens)
    }

    /// 清零累积梯度。
    pub fn zero_grad(&mut self) {
        self.block.zero_grad();
    }

    /// 应用累积梯度。
    pub fn update(&mut self, opts: &mut BlockOptimizers) {
        self.block.update(opts);
    }

    /// 只读访问底层块（调试/查看注意力）。
    pub fn block(&self) -> &AttentionBlock {
        &self.block
    }
}

/// `TernaryDense` 堆叠编码器（对照基线）：`grid³ → hidden → latent`。
pub struct DenseEncoder {
    l1: TernaryDense,
    l2: TernaryDense,
    act: Activation,
    input: usize,
    h: Vec<f32>,
    z: Vec<f32>,
}

impl DenseEncoder {
    /// `grid` 每边体素数，`hidden` 隐层宽，`latent` 输出维。
    pub fn new(grid: usize, hidden: usize, latent: usize, seed: u64) -> Self {
        let input = grid * grid * grid;
        let init = xorshift_init(seed);
        let l1 = TernaryDense::new(hidden, input, init, |_| 0.25, Ste::Identity);
        let init2 = xorshift_init(seed ^ 0xA5A5_5A5A);
        let l2 = TernaryDense::new(latent, hidden, init2, |_| 0.25, Ste::Identity);
        Self {
            l1,
            l2,
            act: Activation::Tanh,
            input,
            h: Vec::new(),
            z: Vec::new(),
        }
    }

    /// latent 维数。
    pub fn latent_dim(&self) -> usize {
        self.l2.rows()
    }

    /// 编码。
    pub fn encode(&mut self, g: &VoxelGrid) -> Vec<f32> {
        assert_eq!(g.cells.len(), self.input, "网格尺寸与编码器不一致");
        let x = g.flatten_f32();
        let pre_h = self.l1.forward(&x);
        self.h = pre_h.iter().map(|&v| self.act.forward(v)).collect();
        let pre_z = self.l2.forward(&self.h);
        self.z = pre_z.iter().map(|&v| self.act.forward(v)).collect();
        self.z.clone()
    }

    /// 反向：返回对输入的梯度（M0 不需要，但保留接线检查）。
    pub fn backward(&mut self, grad_latent: &[f32]) -> Vec<f32> {
        assert_eq!(grad_latent.len(), self.z.len(), "grad_latent 维度不对");
        let grad_pre_z: Vec<f32> = grad_latent
            .iter()
            .zip(&self.z)
            .map(|(&g, &y)| g * self.act.backward(y))
            .collect();
        let grad_h = self.l2.backward(&grad_pre_z);
        let grad_pre_h: Vec<f32> = grad_h
            .iter()
            .zip(&self.h)
            .map(|(&g, &y)| g * self.act.backward(y))
            .collect();
        self.l1.backward(&grad_pre_h)
    }

    /// 清零累积梯度。
    pub fn zero_grad(&mut self) {
        self.l1.zero_grad();
        self.l2.zero_grad();
    }

    /// 应用累积梯度：每层的 latent / scale 各一个优化器。
    pub fn update(
        &mut self,
        l1_latent: &mut dyn trit_learn::Optimizer,
        l1_scale: &mut dyn trit_learn::Optimizer,
        l2_latent: &mut dyn trit_learn::Optimizer,
        l2_scale: &mut dyn trit_learn::Optimizer,
    ) {
        self.l1.update(l1_latent, l1_scale);
        self.l2.update(l2_latent, l2_scale);
    }
}

/// 体素几何头：latent → `grid³` 个体素值（tanh，值域 `[-1,1]`）。
pub struct VoxelHead {
    dense: TernaryDense,
    act: Activation,
    y: Vec<f32>,
}

impl VoxelHead {
    /// `latent` 输入维，`out` 输出体素数。
    pub fn new(latent: usize, out: usize, seed: u64) -> Self {
        let init = xorshift_init(seed ^ 0x5A5A_A5A5);
        Self {
            dense: TernaryDense::new(out, latent, init, |_| 0.25, Ste::Identity),
            act: Activation::Tanh,
            y: Vec::new(),
        }
    }

    /// 前向：返回 `out` 个体素值。
    pub fn forward(&mut self, latent: &[f32]) -> Vec<f32> {
        let pre = self.dense.forward(latent);
        self.y = pre.iter().map(|&v| self.act.forward(v)).collect();
        self.y.clone()
    }

    /// 反向：返回对 latent 的梯度。
    pub fn backward(&mut self, grad_out: &[f32]) -> Vec<f32> {
        assert_eq!(grad_out.len(), self.y.len(), "grad_out 维度不对");
        let grad_pre: Vec<f32> = grad_out
            .iter()
            .zip(&self.y)
            .map(|(&g, &y)| g * self.act.backward(y))
            .collect();
        self.dense.backward(&grad_pre)
    }

    /// 清零累积梯度。
    pub fn zero_grad(&mut self) {
        self.dense.zero_grad();
    }

    /// 应用累积梯度。
    pub fn update(
        &mut self,
        latent_opt: &mut dyn trit_learn::Optimizer,
        scale_opt: &mut dyn trit_learn::Optimizer,
    ) {
        self.dense.update(latent_opt, scale_opt);
    }
}

/// 训练报告。
#[derive(Clone, Debug, PartialEq)]
pub struct TrainReport {
    /// 每步 MSE。
    pub losses: Vec<f32>,
    /// 首步。
    pub first: f32,
    /// 末步。
    pub last: f32,
}

/// 训练注意力自编码器（单样本/小批量顺序）。
pub fn train_attention_ae(
    encoder: &mut AttentionEncoder,
    head: &mut VoxelHead,
    samples: &[VoxelGrid],
    steps: usize,
    lr: f32,
) -> TrainReport {
    assert!(!samples.is_empty(), "至少一个样本");
    let mut block_opts = BlockOptimizers::adam(lr);
    let mut h_latent = Adam::new(lr);
    let mut h_scale = Adam::new(lr);
    let mut losses = Vec::with_capacity(steps);
    for step in 0..steps {
        let g = &samples[step % samples.len()];
        let target = g.flatten_f32();
        encoder.zero_grad();
        head.zero_grad();
        let latent = encoder.encode_flat(g);
        let out = head.forward(&latent);
        let (loss, grad) = trit_learn::loss::mse(&out, &target);
        losses.push(loss);
        let grad_latent = head.backward(&grad);
        encoder.backward_flat(&grad_latent);
        encoder.update(&mut block_opts);
        head.update(&mut h_latent, &mut h_scale);
    }
    let first = *losses.first().unwrap();
    let last = *losses.last().unwrap();
    TrainReport {
        losses,
        first,
        last,
    }
}

/// 训练稠密编码器自编码器（对照基线）。
pub fn train_dense_ae(
    encoder: &mut DenseEncoder,
    head: &mut VoxelHead,
    samples: &[VoxelGrid],
    steps: usize,
    lr: f32,
) -> TrainReport {
    assert!(!samples.is_empty(), "至少一个样本");
    let mut l1_latent = Adam::new(lr);
    let mut l1_scale = Adam::new(lr);
    let mut l2_latent = Adam::new(lr);
    let mut l2_scale = Adam::new(lr);
    let mut h_latent = Adam::new(lr);
    let mut h_scale = Adam::new(lr);
    let mut losses = Vec::with_capacity(steps);
    for step in 0..steps {
        let g = &samples[step % samples.len()];
        let target = g.flatten_f32();
        encoder.zero_grad();
        head.zero_grad();
        let latent = encoder.encode(g);
        let out = head.forward(&latent);
        let (loss, grad) = trit_learn::loss::mse(&out, &target);
        losses.push(loss);
        let grad_latent = head.backward(&grad);
        encoder.backward(&grad_latent);
        encoder.update(&mut l1_latent, &mut l1_scale, &mut l2_latent, &mut l2_scale);
        head.update(&mut h_latent, &mut h_scale);
    }
    let first = *losses.first().unwrap();
    let last = *losses.last().unwrap();
    TrainReport {
        losses,
        first,
        last,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attention_encoder_shape_and_determinism() {
        let g = VoxelGrid::cube(8, 2);
        let mut a = AttentionEncoder::new(8, 4, 32, 7);
        let mut b = AttentionEncoder::new(8, 4, 32, 7);
        let za = a.encode(&g);
        let zb = b.encode(&g);
        assert_eq!(za.len(), 64);
        assert_eq!(za, zb, "同 seed 同输入必须逐位一致");
        assert!(za.iter().all(|v| v.is_finite()));
        assert_eq!(a.tokens(), 8);
    }

    #[test]
    fn dense_encoder_shape_and_determinism() {
        let g = VoxelGrid::cube(8, 2);
        let mut a = DenseEncoder::new(8, 64, 32, 11);
        let mut b = DenseEncoder::new(8, 64, 32, 11);
        let za = a.encode(&g);
        let zb = b.encode(&g);
        assert_eq!(za.len(), 32);
        assert_eq!(za, zb);
        assert!(za.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn attention_ae_loss_decreases() {
        let g = VoxelGrid::cube(8, 2);
        let mut enc = AttentionEncoder::new(8, 4, 32, 7);
        let mut head = VoxelHead::new(enc.flat_dim(), g.cells.len(), 7);
        let r = train_attention_ae(&mut enc, &mut head, &[g], 120, 0.05);
        assert!(r.last < r.first, "loss 未下降: {} -> {}", r.first, r.last);
    }

    #[test]
    fn dense_ae_loss_decreases() {
        let g = VoxelGrid::cube(8, 2);
        let mut enc = DenseEncoder::new(8, 64, 32, 11);
        let mut head = VoxelHead::new(enc.latent_dim(), g.cells.len(), 11);
        let r = train_dense_ae(&mut enc, &mut head, &[g], 120, 0.05);
        assert!(r.last < r.first, "loss 未下降: {} -> {}", r.first, r.last);
    }
}
