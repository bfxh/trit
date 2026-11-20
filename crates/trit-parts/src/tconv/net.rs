mod train;

use super::bn::{AuxCls, Bn, Film};
use super::conv::Conv;

/// FiLM 反向所需：(FiLM 前的 conv 输出, 该类 γ, 类别)。
type FilmState = (Vec<Vec<f32>>, Vec<f32>, Option<usize>);

/// 全卷积栈：隐藏层（可选 BN）+ ReLU，最后一层 sigmoid。
pub struct Net {
    pub layers: Vec<Conv>,
    /// 与 `layers` 等长；只有隐藏层可能启用。
    pub bns: Vec<Option<Bn>>,
    /// 条件通道数（追加在输入后面，常数图）。
    pub cond: usize,
    /// 输出层是否线性（ε-预测需要无界输出；默认 sigmoid 给 x0 用）。
    pub linear_out: bool,
    /// 所有卷积层是否循环填充（可平铺纹理用）。
    pub circular: bool,
    /// 高通（拉普拉斯残差）匹配项的权重；0 = 关。线性输出模式下生效。
    pub highpass: f32,
    /// 每个隐层的 FiLM（`None` = 该层不做条件调制）。
    pub films: Vec<Option<Film>>,
    /// 类别数（0 = 未启用 FiLM）。
    pub nk: usize,
    /// FiLM 的学习率倍数（条件信号弱时需要放大；默认 1）。
    pub film_lr_mul: f32,
    /// 辅助分类头（`None` = 不加）。
    pub aux: Option<AuxCls>,
    /// 辅助分类损失的权重 λ。
    pub aux_w: f32,
    /// 诊断：上一批辅助头自己的 CE 与准确率。
    pub aux_ce: f32,
    pub aux_acc: f32,
    /// 每类一个**输出头**（共享主干）；`None` 的类走基础输出层。
    pub heads: Vec<Option<Conv>>,
    /// 方向性损失：(类别, 目标梯度角弧度, 权重)。`None` = 关。
    pub dir: Option<(usize, f32, f32)>,
    /// 噪声表（v-预测下用于把 v̂ 还原成 x̂0 供方向性损失使用）；空 = 不可用。
    pub abar: Vec<f32>,
    /// 结构张量矩匹配：`(类别, 目标 (Jxx,Jyy,Jxy), λ)`。`None` = 关。
    pub stm: Option<(usize, [f32; 3], f32)>,
    /// coherence 匹配：`(类别, 目标 c*, 目标总能量 T*, λ, 能量项权重)`。`None` = 关。
    pub coh: Option<(usize, f32, f32, f32, f32)>,
}

impl Net {
    pub fn new(cin: usize, hidden: &[usize], seed: u64) -> Result<Self, String> {
        let mut layers = Vec::new();
        let mut c = cin;
        for (i, &h) in hidden.iter().enumerate() {
            layers.push(Conv::new(c, h, seed + i as u64 * 7919)?);
            c = h;
        }
        layers.push(Conv::new(c, 1, seed + hidden.len() as u64 * 7919)?);
        let n = layers.len();
        // 默认不加 BN；`set_bn(true)` 时只给隐藏层
        let bns = (0..n).map(|_| None).collect();
        Ok(Net {
            layers,
            bns,
            cond: 0,
            linear_out: false,
            circular: false,
            highpass: 0.0,
            films: (0..n).map(|_| None).collect(),
            nk: 0,
            film_lr_mul: 1.0,
            aux: None,
            aux_w: 0.0,
            aux_ce: 0.0,
            aux_acc: 0.0,
            heads: Vec::new(),
            dir: None,
            abar: Vec::new(),
            stm: None,
            coh: None,
        })
    }

    /// 开「每类一个输出头」：共享主干，只多最后一层卷积（`nk` 个头）。
    pub fn set_heads(&mut self, nk: usize, seed: u64) {
        if self.layers.len() < 2 {
            return;
        }
        let cin = self.layers[self.layers.len() - 2].cout;
        self.heads = (0..nk)
            .map(|k| Some(Conv::new(cin, 1, seed + 1000 + k as u64).expect("层宽已校验")))
            .collect();
        // 输出头与基础输出层同源初值会打架，这里让基础层退居「空类」
    }

    /// 取该类应使用的输出层（没有专属头就走基础层）。
    fn out_layer(&self, class: Option<usize>) -> Option<&Conv> {
        class
            .and_then(|c| self.heads.get(c))
            .and_then(|h| h.as_ref())
    }

    /// 开辅助分类头：作用在**最后一层隐特征**的全局池化上。
    pub fn set_aux(&mut self, nk: usize) {
        if self.layers.len() < 2 {
            return;
        }
        let c = self.layers[self.layers.len() - 2].cout;
        self.aux = Some(AuxCls::new(nk, c));
    }

    /// 开关 FiLM 条件：`nk` 个类 + 1 个空类，只作用在隐藏层。
    pub fn set_film(&mut self, nk: usize) {
        self.nk = nk;
        let n = self.layers.len();
        for i in 0..n {
            self.films[i] = if i + 1 < n {
                Some(Film::new(nk, self.layers[i].cout))
            } else {
                None
            };
        }
    }

    /// 开关循环填充（可平铺纹理；零填充会让边界变平，接缝比 <1）。
    pub fn set_circular(&mut self, on: bool) {
        self.circular = on;
        for l in self.layers.iter_mut() {
            l.circular = on;
        }
    }

    /// 全局膨胀率（含输出头）：感受野 = 2·dil·层数 + 1，算力不变。
    pub fn set_dilation(&mut self, d: usize) {
        let d = d.max(1);
        for l in self.layers.iter_mut() {
            l.dil = d;
        }
        for h in self.heads.iter_mut().flatten() {
            h.dil = d;
        }
    }

    /// 开关 BN（隐藏层；最后一层不加）。
    pub fn set_bn(&mut self, on: bool) {
        let n = self.layers.len();
        for (i, b) in self.bns.iter_mut().enumerate() {
            *b = if on && i + 1 < n {
                Some(Bn::new(self.layers[i].cout))
            } else {
                None
            };
        }
    }

    fn input(&self, img: &[f32], cond: Option<usize>, w: usize, h: usize) -> Vec<Vec<f32>> {
        assert!(
            img.len() == w.saturating_mul(h),
            "img 长度必须等于 w×h（{w}×{h}，收到 {}）",
            img.len()
        );
        let mut x = vec![img.to_vec()];
        for k in 0..self.cond {
            x.push(vec![(cond == Some(k)) as u8 as f32; w * h]);
        }
        x
    }

    pub fn forward(&self, img: &[f32], cond: Option<usize>, w: usize, h: usize) -> Vec<f32> {
        let x = self.input(img, cond, w, h);
        self.forward_x(&x, w, h)
    }

    /// 直接吃一组通道图（扩散去噪器要把 `t/T` 当第二通道，`input` 造不出来）。
    pub fn forward_x(&self, x: &[Vec<f32>], w: usize, h: usize) -> Vec<f32> {
        self.forward_xc(x, None, w, h)
    }

    /// 带类别条件的前向（FiLM）。
    pub fn forward_xc(&self, x: &[Vec<f32>], class: Option<usize>, w: usize, h: usize) -> Vec<f32> {
        let mut x = x.to_vec();
        for i in 0..self.layers.len() {
            let mut y = if i + 1 == self.layers.len() {
                match self.out_layer(class) {
                    Some(hd) => hd.forward(&x, w, h),
                    None => self.layers[i].forward(&x, w, h),
                }
            } else {
                self.layers[i].forward(&x, w, h)
            };
            if let Some(f) = &self.films[i] {
                let (g, bb) = (f.gamma(class), f.beta(class));
                for (ch, v) in y.iter_mut().enumerate() {
                    let (gi, bi) = (g[ch], bb[ch]);
                    for t in v.iter_mut() {
                        *t = gi * *t + bi;
                    }
                }
            }
            if let Some(bn) = &self.bns[i] {
                // 推理也用**输入自身的统计**（单图 InstanceNorm 语义），
                // 否则训练（批统计）与评测（running stats）不一致，BN 版会直接崩。
                let mut b = bn.clone();
                y = b.forward(&y, w * h, true).0;
            }
            let last = i + 1 == self.layers.len();
            let linear = last && self.linear_out;
            for ch in y.iter_mut() {
                for v in ch.iter_mut() {
                    *v = if linear {
                        *v
                    } else if last {
                        1.0 / (1.0 + (-*v).exp())
                    } else {
                        v.max(0.0)
                    };
                }
            }
            x = y;
        }
        x.pop().unwrap()
    }
}
