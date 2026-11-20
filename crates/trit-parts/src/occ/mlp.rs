//! 三值 MLP：权重取 {−1, 0, +1}，缩放交给输出层，STE 反向。

/// 两层三值 MLP（隐藏层 ReLU，输出 logit），STE 反向。
pub struct Mlp {
    pub nin: usize,
    pub nhid: usize,
    /// 实际值（三值 × 每层缩放）；训练时直接更新，量化在 `q()` 里取。
    pub(crate) w1: Vec<f32>,
    pub(crate) b1: Vec<f32>,
    pub(crate) w2: Vec<f32>,
    pub(crate) b2: f32,
}

/// 三值化：有效权重 ∈ {−1, 0, +1}（每层缩放交给输出层的 f32 权重吸收，
/// 否则 0.02 量级的隐藏激活会把 logit 动态范围压死）。
fn quant(v: f32) -> f32 {
    v.round().clamp(-1.0, 1.0)
}

impl Mlp {
    pub fn new(nin: usize, nhid: usize, seed: u64) -> Self {
        // xorshift 状态不能为零。旧写法 `seed | 1` 会让**偶数种子与下一个奇数种子撞成同一初始化**
        // （实测 seed 2 与 3 训出的模型逐字节相同）；奇数种子保持原值，偶数种子先混合一次。
        let mut r = if seed.is_multiple_of(2) {
            seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1
        } else {
            seed
        };
        let mut next = || {
            r ^= r << 13;
            r ^= r >> 7;
            r ^= r << 17;
            ((r % 2001) as f32 - 1000.0) / 2500.0
        };
        let n1 = nin * nhid;
        let n2 = nhid;
        Mlp {
            nin,
            nhid,
            w1: (0..n1).map(|_| next() * 5.0).collect(),
            b1: vec![0.0; nhid],
            w2: (0..n2).map(|_| next()).collect(),
            b2: 0.0,
        }
    }

    /// 前向：返回 (logit, 隐藏层)。
    pub fn forward(&self, x: &[f32]) -> (f32, Vec<f32>) {
        let mut h = vec![0.0f32; self.nhid];
        for (j, hj) in h.iter_mut().enumerate() {
            let mut s = self.b1[j];
            for (i, xi) in x.iter().enumerate().take(self.nin) {
                s += quant(self.w1[i * self.nhid + j]) * xi;
            }
            *hj = s.max(0.0);
        }
        let mut z = self.b2;
        // 输出层保持 f32（低比特网络的通行做法：首/末层不全三值，否则 logit 动态范围被压死）
        for (j, hj) in h.iter().enumerate() {
            z += self.w2[j] * hj;
        }
        (z, h)
    }

    /// 单样本 BCE + STE 梯度更新（SGD）。
    pub fn train_step(&mut self, x: &[f32], y: f32, lr: f32) -> f32 {
        let (z, h) = self.forward(x);
        let p = 1.0 / (1.0 + (-z).exp());
        let eps = 1e-7;
        let loss = -(y * (p + eps).ln() + (1.0 - y) * (1.0 - p + eps).ln());
        let dz = p - y;
        // 输出层
        for (w, hj) in self.w2.iter_mut().zip(h.iter()) {
            *w -= lr * dz * *hj;
        }
        self.b2 -= lr * dz;
        // 隐藏层（STE：梯度穿过量化）
        for (j, hj) in h.iter().enumerate() {
            if *hj <= 0.0 {
                continue;
            }
            let dh = dz * self.w2[j];
            for (i, xi) in x.iter().enumerate().take(self.nin) {
                self.w1[i * self.nhid + j] -= lr * dh * xi;
            }
            self.b1[j] -= lr * dh;
        }
        loss
    }

    /// 存盘（文本：首行 dims，随后 w1/b1/w2/b2）。
    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        let mut out = format!("OCC1 {} {}\n", self.nin, self.nhid);
        let push = |out: &mut String, v: &[f32]| {
            for x in v {
                out.push_str(&format!("{x} "));
            }
            out.push('\n');
        };
        push(&mut out, &self.w1);
        push(&mut out, &self.b1);
        push(&mut out, &self.w2);
        out.push_str(&format!("{}\n", self.b2));
        std::fs::write(path, out).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut lines = text.lines();
        let head = lines.next().ok_or("模型文件为空")?;
        let mut it = head.split_whitespace();
        if it.next() != Some("OCC1") {
            return Err("不是 OCC1 模型".into());
        }
        let nin: usize = it
            .next()
            .ok_or("缺 nin")?
            .parse()
            .map_err(|_| "nin 解析失败")?;
        let nhid: usize = it
            .next()
            .ok_or("缺 nhid")?
            .parse()
            .map_err(|_| "nhid 解析失败")?;
        let mut take = |n: usize, what: &str| -> Result<Vec<f32>, String> {
            let line = lines.next().ok_or(format!("缺 {what}"))?;
            let v: Vec<f32> = line
                .split_whitespace()
                .filter_map(|x| x.parse().ok())
                .collect();
            if v.len() != n {
                return Err(format!("{what} 长度 {} != {n}", v.len()));
            }
            Ok(v)
        };
        let w1 = take(nin * nhid, "w1")?;
        let b1 = take(nhid, "b1")?;
        let w2 = take(nhid, "w2")?;
        let b2: f32 = lines
            .next()
            .ok_or("缺 b2")?
            .trim()
            .parse()
            .map_err(|_| "b2 解析失败")?;
        Ok(Mlp {
            nin,
            nhid,
            w1,
            b1,
            w2,
            b2,
        })
    }

    /// 权重三值化后的稀疏度（0 的占比），用于报告。
    pub fn sparsity(&self) -> f32 {
        let z = self.w1.iter().filter(|v| quant(**v) == 0.0).count();
        z as f32 / self.w1.len().max(1) as f32
    }
}
