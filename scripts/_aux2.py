import io

p = 'crates/trit-parts/src/tconv.rs'
s = io.open(p, encoding='utf-8', newline='').read()

# 1) 累加器
old_acc = """        // FiLM 的梯度按「层 × 类 × 通道」累加"""
new_acc = """        // 辅助分类头的梯度（w: nk×c，b: nk）
        let mut acc_aw: Vec<f32> = self.aux.as_ref().map(|a| vec![0.0; a.w.len()]).unwrap_or_default();
        let mut acc_ab: Vec<f32> = self.aux.as_ref().map(|a| vec![0.0; a.b.len()]).unwrap_or_default();
        // FiLM 的梯度按「层 × 类 × 通道」累加"""
assert s.count(old_acc) == 1
s = s.replace(old_acc, new_acc, 1)

# 2) 前向：辅助分类（在最后一层隐特征上）
old_fwd_end = """            let pred = &x[0];
            let mut d = vec![vec![0.0f32; n]; 1];"""
new_fwd_end = """            // 辅助分类：对「最后一层隐特征」做全局平均池化
            let mut aux_dpool: Option<Vec<f32>> = None;
            let mut aux_feat: Option<&Vec<Vec<f32>>> = None;
            if let (Some(a), Some(cls)) = (self.aux.as_ref(), class) {
                if *cls < a.nk && self.layers.len() >= 2 {
                    let feats = &ins[self.layers.len() - 1];
                    let mut pool = vec![0.0f32; a.c];
                    for (ch, v) in feats.iter().enumerate() {
                        pool[ch] = v.iter().sum::<f32>() / n as f32;
                    }
                    let logits = a.logits(&pool);
                    let mx = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                    let exps: Vec<f32> = logits.iter().map(|v| (v - mx).exp()).collect();
                    let z: f32 = exps.iter().sum::<f32>().max(1e-9);
                    let probs: Vec<f32> = exps.iter().map(|v| v / z).collect();
                    loss += f64::from(self.aux_w * -(probs.get(*cls).copied().unwrap_or(1e-6).max(1e-6).ln()));
                    let mut dlog = probs.clone();
                    dlog[*cls] -= 1.0;
                    for k in 0..a.nk {
                        acc_ab[k] += self.aux_w * dlog[k];
                        for i in 0..a.c {
                            acc_aw[k * a.c + i] += self.aux_w * dlog[k] * pool[i];
                        }
                    }
                    // dL/dpool → 回传到每个像素
                    let mut dpool = vec![0.0f32; a.c];
                    for i in 0..a.c {
                        dpool[i] = (0..a.nk)
                            .map(|k| self.aux_w * dlog[k] * a.w[k * a.c + i])
                            .sum();
                    }
                    aux_dpool = Some(dpool);
                    aux_feat = Some(feats);
                }
            }

            let pred = &x[0];
            let mut d = vec![vec![0.0f32; n]; 1];"""
assert s.count(old_fwd_end) == 1
s = s.replace(old_fwd_end, new_fwd_end, 1)

# 3) 反向：在最后一层隐层（li == len-2）的 ReLU 掩码之后，加上 aux 梯度
old_bwd = """                if let (Some((pre, g, cls)), Some(ag), Some(ab)) = ("""
new_bwd = """                // 辅助分类头对「该层激活输出」的梯度：只在 ReLU 未截断处有效
                if let (Some(dpool), Some(feats)) = (aux_dpool.as_ref(), aux_feat.as_ref()) {
                    if li + 2 == self.layers.len() {
                        for (ch, row) in feats.iter().enumerate() {
                            let g = dpool[ch] / n as f32;
                            for (i, v) in row.iter().enumerate() {
                                if *v > 0.0 {
                                    d[ch][i] += g;
                                }
                            }
                        }
                    }
                }
                if let (Some((pre, g, cls)), Some(ag), Some(ab)) = ("""
assert s.count(old_bwd) == 1
s = s.replace(old_bwd, new_bwd, 1)

# 4) 批末更新 aux
old_apply = """        for li in 0..self.layers.len() {
            if let (Some(f), Some(ag), Some(ab)) ="""
new_apply = """        if let Some(a) = self.aux.as_mut() {
            for i in 0..a.w.len() {
                a.w[i] -= lr * acc_aw[i] / k as f32;
            }
            for i in 0..a.b.len() {
                a.b[i] -= lr * acc_ab[i] / k as f32;
            }
        }
        for li in 0..self.layers.len() {
            if let (Some(f), Some(ag), Some(ab)) ="""
assert s.count(old_apply) == 1
s = s.replace(old_apply, new_apply, 1)

io.open(p, 'w', encoding='utf-8', newline='').write(s)
print("patched aux train")
