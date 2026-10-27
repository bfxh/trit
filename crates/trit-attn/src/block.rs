//! 可训练的三值注意力块：注意力 + 残差 + FFN 的训练集成（M0）。
//!
//! 结构（pre-logit 单头，`T` 为序列长度、`d` 为 [`AttentionBlock::dim`]）：
//!
//! ```text
//! Q = X·Wq  K = X·Wk  V = X·Wv          ← 三值 QAT 投影（per-row scale + STE）
//! A = softmax(Q·Kᵀ/√d)                  ← 注意力权重（T×T）
//! H = X + A·V                           ← 残差一
//! Y = H + act(H·W1)·W2                  ← 残差二（三值 FFN，无偏置）
//! ```
//!
//! 前向始终用三值权重（QAT）：训练结束不存在「导出再量化」的精度落差，
//! 只有 per-row scale 是 f32。反向是把 [`Ste`] 直通到 latent 的完整链式法则：
//! softmax 用 [`softmax_backward`]，激活用 [`Activation::backward`]，
//! 残差把梯度分给两条路径，权重/尺度梯度在层内累积、由 [`BlockOptimizers`] 统一应用。
//!
//! M0 边界：单头、无偏置、无 LayerNorm/KV-cache/因果掩码（见 ROADMAP §3）。

use trit_learn::{Activation, Optimizer, Ste, TernaryDense, softmax, softmax_backward};

/// 注意力块里的五个投影层（更新顺序固定）。
const LAYERS: usize = 5;

/// xorshift 驱动的 latent 初始化：随机符号、幅值 `U(0.1, 0.3)`。
/// 配 scale 0.25 时 `|w/s| ∈ [0.4, 1.2]`——大部分权重落到 `±1`、小部分落在
/// `0`，三值权重混合起步。若用零均值小扰动起步，`q` 会全为 0，
/// 前向对 latent 是分片常量，训练要空转等权重漂出死区。
fn xorshift_init(state: &mut u64) -> impl FnMut(usize, usize) -> f32 + '_ {
    move |_, _| {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        let magnitude = 0.1 + ((*state % 200) as f32) / 1000.0;
        if *state & 1 == 0 {
            magnitude
        } else {
            -magnitude
        }
    }
}

/// 五层各自的参数各配一个优化器实例（latent 与 scale 分开建状态）。
pub struct BlockOptimizers {
    latent: Vec<Box<dyn Optimizer>>,
    scale: Vec<Box<dyn Optimizer>>,
}

impl BlockOptimizers {
    /// 全部参数用带动量 SGD。
    pub fn sgd(lr: f32, momentum: f32) -> Self {
        Self::build(
            || Box::new(trit_learn::opt::Sgd::new(lr, momentum)),
            || Box::new(trit_learn::opt::Sgd::new(lr, momentum)),
        )
    }

    /// 全部参数用 Adam。
    pub fn adam(lr: f32) -> Self {
        Self::build(
            || Box::new(trit_learn::opt::Adam::new(lr)),
            || Box::new(trit_learn::opt::Adam::new(lr)),
        )
    }

    fn build(
        mut latent: impl FnMut() -> Box<dyn Optimizer>,
        mut scale: impl FnMut() -> Box<dyn Optimizer>,
    ) -> Self {
        Self {
            latent: (0..LAYERS).map(|_| latent()).collect(),
            scale: (0..LAYERS).map(|_| scale()).collect(),
        }
    }
}

/// 可训练的三值注意力块。
pub struct AttentionBlock {
    /// token 维数（= 投影输入/输出维数）。
    dim: usize,
    /// FFN 隐层宽度。
    hidden: usize,
    act: Activation,
    wq: TernaryDense,
    wk: TernaryDense,
    wv: TernaryDense,
    f1: TernaryDense,
    f2: TernaryDense,
    // —— forward 缓存（backward 需要；每次 forward 覆盖）——
    seq_len: usize,
    q: Vec<f32>,
    k: Vec<f32>,
    v: Vec<f32>,
    /// 注意力权重 `T×T`（行和为 1）。
    a: Vec<f32>,
    /// 残差一的输出 `H = X + A·V`。
    h: Vec<f32>,
    /// FFN 激活输出（`T×hidden`）。
    f_act: Vec<f32>,
}

impl AttentionBlock {
    /// 新建：`seed` 驱动确定性初始化（latent 随机符号、幅值 `U(0.1, 0.3)`，
    /// scale 0.25，让三值权重以混合 `{-1,0,+1}` 起步）。
    pub fn new(dim: usize, hidden: usize, act: Activation, ste: Ste, seed: u64) -> Self {
        assert!(dim > 0 && hidden > 0, "dim/hidden 必须非零");
        let mut state = seed | 1;
        Self {
            dim,
            hidden,
            act,
            wq: TernaryDense::new(dim, dim, xorshift_init(&mut state), |_| 0.25, ste),
            wk: TernaryDense::new(dim, dim, xorshift_init(&mut state), |_| 0.25, ste),
            wv: TernaryDense::new(dim, dim, xorshift_init(&mut state), |_| 0.25, ste),
            f1: TernaryDense::new(hidden, dim, xorshift_init(&mut state), |_| 0.25, ste),
            f2: TernaryDense::new(dim, hidden, xorshift_init(&mut state), |_| 0.25, ste),
            seq_len: 0,
            q: Vec::new(),
            k: Vec::new(),
            v: Vec::new(),
            a: Vec::new(),
            h: Vec::new(),
            f_act: Vec::new(),
        }
    }

    /// token 维数。
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// FFN 隐层宽度。
    pub fn hidden(&self) -> usize {
        self.hidden
    }

    /// 最近一次 forward 的序列长度。
    pub fn seq_len(&self) -> usize {
        self.seq_len
    }

    /// 最近一次 forward 的注意力权重（`T×T` 行主序，行和为 1）。
    pub fn attention(&self) -> &[f32] {
        &self.a
    }

    fn layers_mut(&mut self) -> [&mut TernaryDense; LAYERS] {
        [
            &mut self.wq,
            &mut self.wk,
            &mut self.wv,
            &mut self.f1,
            &mut self.f2,
        ]
    }

    /// 前向：`seq` 是 `T×dim` 行主序扁平数组，返回 `T×dim`。
    pub fn forward(&mut self, seq: &[f32]) -> Vec<f32> {
        assert!(
            !seq.is_empty() && seq.len().is_multiple_of(self.dim),
            "序列长度 {} 必须是 dim={} 的非零整数倍",
            seq.len(),
            self.dim
        );
        let t = seq.len() / self.dim;
        let d = self.dim;
        let q = self.wq.forward(seq);
        let k = self.wk.forward(seq);
        let v = self.wv.forward(seq);
        // logits[t,s] = Q[t]·K[s]/√d；A 每行 softmax。
        let scale = (d as f32).sqrt();
        let mut a = vec![0.0f32; t * t];
        for i in 0..t {
            let row = &q[i * d..(i + 1) * d];
            let logits: Vec<f32> = (0..t)
                .map(|s| {
                    row.iter()
                        .zip(&k[s * d..(s + 1) * d])
                        .map(|(&qi, &ki)| qi * ki)
                        .sum::<f32>()
                        / scale
                })
                .collect();
            let probs = softmax(&logits);
            a[i * t..(i + 1) * t].copy_from_slice(&probs);
        }
        // H = X + A·V（残差一）。
        let mut h = seq.to_vec();
        for i in 0..t {
            for s in 0..t {
                let weight = a[i * t + s];
                if weight == 0.0 {
                    continue;
                }
                for (slot, &vc) in h[i * d..(i + 1) * d].iter_mut().zip(&v[s * d..(s + 1) * d]) {
                    *slot += weight * vc;
                }
            }
        }
        // FFN：act(H·W1)·W2，再残差二。
        let f_pre = self.f1.forward(&h);
        let f_act: Vec<f32> = f_pre.iter().map(|&x| self.act.forward(x)).collect();
        let f_out = self.f2.forward(&f_act);
        let mut y = h.clone();
        for (slot, &fc) in y.iter_mut().zip(f_out.iter()) {
            *slot += fc;
        }
        self.seq_len = t;
        self.q = q;
        self.k = k;
        self.v = v;
        self.a = a;
        self.h = h;
        self.f_act = f_act;
        y
    }

    /// 反向：`grad_out` 是对最近一次 forward 输出（`T×dim`）的梯度，
    /// 返回对输入序列的梯度；权重/尺度梯度累积到各层，等 [`AttentionBlock::update`] 应用。
    pub fn backward(&mut self, grad_out: &[f32]) -> Vec<f32> {
        let t = self.seq_len;
        let d = self.dim;
        assert_eq!(
            grad_out.len(),
            t * d,
            "grad_out 长度必须是 T×dim={}，实际 {}",
            t * d,
            grad_out.len()
        );
        // 残差二：dY 同时流向 H 和 FFN。
        // FFN 第二层：g_f_act = dY·W2（经 STE）。
        let g_f_act = self.f2.backward(grad_out);
        // 激活：g_f_pre = g_f_act ⊙ act'(f_act)。
        let g_f_pre: Vec<f32> = g_f_act
            .iter()
            .zip(self.f_act.iter())
            .map(|(&g, &y)| g * self.act.backward(y))
            .collect();
        // FFN 第一层：g_h_ffn = g_f_pre·W1。
        let g_h_ffn = self.f1.backward(&g_f_pre);
        // H 收到的总梯度 = dY（残差二）+ FFN 支路。
        let mut dh = grad_out.to_vec();
        for (slot, &g) in dh.iter_mut().zip(g_h_ffn.iter()) {
            *slot += g;
        }
        // 注意力输出 = A·V：dV[s] = Σ_t A[t,s]·dH[t]；dA[t,s] = dH[t]·V[s]。
        let mut dv = vec![0.0f32; t * d];
        let mut da = vec![0.0f32; t * t];
        for i in 0..t {
            for s in 0..t {
                let weight = self.a[i * t + s];
                let dh_i = &dh[i * d..(i + 1) * d];
                let v_s = &self.v[s * d..(s + 1) * d];
                for c in 0..d {
                    dv[s * d + c] += weight * dh_i[c];
                    da[i * t + s] += dh_i[c] * v_s[c];
                }
            }
        }
        // softmax 反向得到对 logits 的梯度（每行独立）。
        let mut d_logit = vec![0.0f32; t * t];
        for i in 0..t {
            let probs = &self.a[i * t..(i + 1) * t];
            let grads = &da[i * t..(i + 1) * t];
            let row = softmax_backward(probs, grads);
            d_logit[i * t..(i + 1) * t].copy_from_slice(&row);
        }
        // logits = Q·Kᵀ/√d：dQ[i] = Σ_s dL[i,s]·K[s]/√d；dK[s] = Σ_i dL[i,s]·Q[i]/√d。
        let inv_scale = 1.0 / (d as f32).sqrt();
        let mut dq = vec![0.0f32; t * d];
        let mut dk = vec![0.0f32; t * d];
        for i in 0..t {
            for s in 0..t {
                let g = d_logit[i * t + s] * inv_scale;
                if g == 0.0 {
                    continue;
                }
                for c in 0..d {
                    dq[i * d + c] += g * self.k[s * d + c];
                    dk[s * d + c] += g * self.q[i * d + c];
                }
            }
        }
        // 三个投影把梯度送回输入，X 还吃一份残差梯度。
        let gx_q = self.wq.backward(&dq);
        let gx_k = self.wk.backward(&dk);
        let gx_v = self.wv.backward(&dv);
        let mut gx = dh;
        for (slot, g) in gx
            .iter_mut()
            .zip(gx_q.iter().zip(gx_k.iter()).zip(gx_v.iter()))
        {
            let ((gq, gk), gv) = g;
            *slot += gq + gk + gv;
        }
        gx
    }

    /// 清零全部层累积梯度（新一步开始时调用）。
    pub fn zero_grad(&mut self) {
        for layer in self.layers_mut() {
            layer.zero_grad();
        }
    }

    /// 用 [`BlockOptimizers`] 应用全部累积梯度（scale 夹回正下界）。
    ///
    /// 梯度不清零——调用方用 [`AttentionBlock::zero_grad`] 开新一步。
    pub fn update(&mut self, opts: &mut BlockOptimizers) {
        assert_eq!(
            opts.latent.len(),
            LAYERS,
            "BlockOptimizers 必须带 {LAYERS} 组优化器"
        );
        for (layer, (latent_opt, scale_opt)) in self
            .layers_mut()
            .into_iter()
            .zip(opts.latent.iter_mut().zip(opts.scale.iter_mut()))
        {
            layer.update(latent_opt.as_mut(), scale_opt.as_mut());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trit_learn::loss::mse;

    struct Rng(u64);

    impl Rng {
        fn f32(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            ((self.0 % 2000) as f32) / 1000.0 - 1.0
        }
    }

    fn block(seed: u64) -> AttentionBlock {
        AttentionBlock::new(3, 4, Activation::Tanh, Ste::Identity, seed)
    }

    #[test]
    fn attention_rows_sum_to_one() {
        let mut b = block(7);
        let seq: Vec<f32> = (0..2 * 3).map(|_| 0.5).collect();
        let _ = b.forward(&seq);
        let t = b.seq_len();
        for i in 0..t {
            let row = &b.attention()[i * t..(i + 1) * t];
            let sum: f32 = row.iter().sum();
            assert!((sum - 1.0).abs() < 1e-5, "行 {i} 和为 {sum}");
            assert!(row.iter().all(|&p| (0.0..=1.0).contains(&p)));
        }
    }

    #[test]
    fn forward_matches_independent_attention_reference() {
        // 与 scores.rs 的独立实现（单查询视角）对拍：取第 2 行的 Q 作为查询，
        // 全部 K/V 作键值，attention() 结果应等于本块输出第 2 行减去残差与 FFN 支路前
        // 的 A·V 部分——直接比 A·V：用同一批 Q/K/V 重算。
        let mut b = block(11);
        let seq: Vec<f32> = (0..3 * 3).map(|i| (i % 5) as f32 - 2.0).collect();
        let _ = b.forward(&seq);
        let t = b.seq_len();
        let d = b.dim();
        // 从块的输出反解不出 A·V，所以这里改为验证：把 H 喂进 FFN 前，
        // 先用 scores::attention 独立重算 A·V，再与 H - X 比对。
        let q = b.q.clone();
        let k = b.k.clone();
        let v = b.v.clone();
        for i in 0..t {
            let query = &q[i * d..(i + 1) * d];
            let keys: Vec<Vec<f32>> = (0..t).map(|s| k[s * d..(s + 1) * d].to_vec()).collect();
            let values: Vec<Vec<f32>> = (0..t).map(|s| v[s * d..(s + 1) * d].to_vec()).collect();
            let reference = crate::scores::attention(query, &keys, &values);
            let x_i = &seq[i * d..(i + 1) * d];
            let h_i = &b.h[i * d..(i + 1) * d];
            for (c, &hc) in h_i.iter().enumerate() {
                assert!(
                    (hc - (x_i[c] + reference[c])).abs() < 1e-4,
                    "行 {i} 列 {c}：H-X 与独立注意力实现差 {}",
                    hc - (x_i[c] + reference[c])
                );
            }
        }
    }

    #[test]
    fn input_gradients_match_finite_differences() {
        // 端到端 wiring 检查：五个层、两条残差、softmax、激活、1/√d——
        // 任何一处接反，dX 都对不上数值差分。
        let (t, d) = (2usize, 3usize);
        let mut rng = Rng(0xFEED);
        let mut b = block(0xABCD);
        let seq: Vec<f32> = (0..t * d).map(|_| rng.f32()).collect();
        let target: Vec<f32> = (0..t * d).map(|_| rng.f32()).collect();
        let loss_of = |b: &mut AttentionBlock, seq: &[f32]| -> f32 {
            let y = b.forward(seq);
            mse(&y, &target).0
        };
        let y = b.forward(&seq);
        let (_, grad_out) = mse(&y, &target);
        let gx = b.backward(&grad_out);
        let h = 1e-3;
        for idx in [0usize, 2, 3, 5] {
            let mut up = seq.clone();
            up[idx] += h;
            let mut down = seq.clone();
            down[idx] -= h;
            let numeric = (loss_of(&mut b, &up) - loss_of(&mut b, &down)) / (2.0 * h);
            assert!(
                (gx[idx] - numeric).abs() < 2e-3,
                "dX[{idx}]：解析 {} vs 数值 {numeric}",
                gx[idx]
            );
        }
    }

    #[test]
    fn relu_variant_also_backprops() {
        let (t, d) = (2usize, 2usize);
        let mut b = AttentionBlock::new(d, 3, Activation::Relu, Ste::Identity, 42);
        let seq = [0.5, -0.5, 1.0, 0.25];
        let target = [0.0, 1.0, -1.0, 0.5];
        let y = b.forward(&seq);
        let (_, grad_out) = mse(&y, &target);
        let gx = b.backward(&grad_out);
        let loss_of = |b: &mut AttentionBlock, s: &[f32]| mse(&b.forward(s), &target).0;
        let h = 1e-3;
        for idx in 0..t * d {
            let mut up = seq;
            up[idx] += h;
            let mut down = seq;
            down[idx] -= h;
            let numeric = (loss_of(&mut b, &up) - loss_of(&mut b, &down)) / (2.0 * h);
            assert!(
                (gx[idx] - numeric).abs() < 2e-3,
                "dX[{idx}]：解析 {} vs 数值 {numeric}",
                gx[idx]
            );
        }
    }

    #[test]
    fn weight_gradients_accumulate_over_batch() {
        // 对同一序列 backward 两次，权重梯度应翻倍（wiring 的批累积面）。
        let mut b = block(5);
        let seq = [0.3, -0.4, 0.6, 0.1, -0.2, 0.8];
        let target = [1.0, -1.0, 0.0, 0.5, -0.5, 0.0];
        let y = b.forward(&seq);
        let (_, grad_out) = mse(&y, &target);
        b.backward(&grad_out);
        let once: Vec<f32> = b.wq.grad_latent().to_vec();
        b.zero_grad();
        let y = b.forward(&seq);
        let (_, grad_out) = mse(&y, &target);
        b.backward(&grad_out);
        b.backward(&grad_out);
        for (twice, one) in b.wq.grad_latent().iter().zip(once.iter()) {
            assert!((twice - 2.0 * one).abs() < 1e-5, "{twice} vs 2·{one}");
        }
    }

    #[test]
    fn sgd_step_reduces_loss_on_fixed_batch() {
        // 小步训练必须在固定批上降 loss（优化器接线的端到端检查）。
        let mut b = block(3);
        let seq = [1.0, -1.0, 0.5, -0.5, 0.25, 0.75];
        let target = [-0.5, 0.5, 1.0, 0.0, -1.0, 0.5];
        let mut opts = BlockOptimizers::sgd(0.05, 0.0);
        let first = mse(&b.forward(&seq), &target).0;
        for _ in 0..40 {
            b.zero_grad();
            let y = b.forward(&seq);
            let (_, grad_out) = mse(&y, &target);
            let _ = b.backward(&grad_out);
            b.update(&mut opts);
        }
        let last = mse(&b.forward(&seq), &target).0;
        assert!(last < first, "loss 未下降：{first} → {last}");
    }

    #[test]
    #[should_panic(expected = "整数倍")]
    fn forward_rejects_bad_sequence_length() {
        let mut b = block(1);
        let _ = b.forward(&[1.0, 2.0]);
    }
}
