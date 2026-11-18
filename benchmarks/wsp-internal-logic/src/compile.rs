//! M0f：DFA → 三值网络 的编译器（Tracr 反向闭环的前半）。
//!
//! 展开 `max_len` 步，每步 2 层：
//!
//! ```text
//! AND 层：对每个 (状态 q, 符号 a) 算 sign(state_q + (±1)·sym_t − 1)
//! OR  层：new_state_j = OR_{δ(q,a)=j} 该项
//! ```
//!
//! **OR 的任意扇入**：`OR(x1..xn) = sign(Σxi + (n−1))`，而 `n−1` 由 `n−1` 个恒为
//! `Pos` 的常量单元供给（权重只能是 ±1，偏置只能 ±1/0，但常量单元不受限）。
//! 因此每步仍是 2 层，不需要树形归约。
//!
//! `a=1` 时接 `+sym`、`a=0` 时接 `−sym`；状态 one-hot 取 `{Neg, Pos}`。
//! 输入每位置 2 维 `(sym, is_end)`：`0 → (Neg, Neg)`、`1 → (Pos, Neg)`、填充 → `(Zero, Pos)`。
//! 注意这里**不用** `word::encode`——那一版把 0 编成 `Zero`（为线性阈值忽略填充），
//! 而阈值门需要 `{±1}`，两个编码不能混用。
//!
//! 未实现：End 冻结（`End` 会被当符号继续执行转移）。闭环测试因此只在定长域上比较。

use trit_wspace::{Layer, Net, Trit};

use crate::dfa::Dfa;

/// 编译器用的符号编码（与 `word::encode` 不同：0 编成 `Neg`）。
#[must_use]
pub fn encode_sym(w: &[Trit], max_len: usize) -> Vec<Trit> {
    let mut out = Vec::with_capacity(max_len * 2);
    for i in 0..max_len {
        match w.get(i) {
            Some(Trit::Pos) => {
                out.push(Trit::Pos);
                out.push(Trit::Neg);
            }
            Some(Trit::Neg) => {
                out.push(Trit::Neg);
                out.push(Trit::Neg);
            }
            _ => {
                out.push(Trit::Zero);
                out.push(Trit::Pos);
            }
        }
    }
    out
}

/// 把 DFA 编译成展开 `max_len` 步的三值前馈网络。
pub fn compile(dfa: &Dfa, max_len: usize) -> Result<Net, String> {
    let k = dfa.states;
    let dim_in = max_len * 2;

    // 每个目标状态的转移源（用 `q*2 + a` 编码）。
    let sources: Vec<Vec<usize>> = (0..k)
        .map(|j| {
            (0..k)
                .flat_map(|q| (0..2).map(move |a| (q, a)))
                .filter(|(q, a)| dfa.delta[*q][*a] == j)
                .map(|(q, a)| q * 2 + a)
                .collect()
        })
        .collect();
    let accepting: Vec<usize> = (0..k).filter(|j| dfa.accept[*j]).collect();

    // 需要多少个常量单元：`max(源数, 接受数) − 1`。
    let max_src = sources.iter().map(Vec::len).max().unwrap_or(0);
    let c = max_src.max(accepting.len()).saturating_sub(1);

    let or_w = k + dim_in + c;
    let and_w = 2 * k + dim_in + c;

    let mut layers: Vec<Layer> = Vec::new();

    // 层 0：初始化状态（bias 给出 one-hot）+ 传递输入 + 常量。
    let mut trits = vec![0i8; or_w * dim_in];
    let mut bias = vec![0i8; or_w];
    for i in 0..k {
        bias[i] = if i == dfa.start { 1 } else { -1 };
    }
    for i in 0..dim_in {
        trits[(k + i) * dim_in + i] = 1;
    }
    for j in 0..c {
        bias[k + dim_in + j] = 1;
    }
    layers.push(Layer::new(or_w, dim_in, &trits, &bias).map_err(|e| e.to_string())?);

    for step in 0..max_len {
        // AND 层：2k 个 AND 项 + dim_in 个输入传递 + c 个常量。
        let mut trits = vec![0i8; and_w * or_w];
        let mut bias = vec![0i8; and_w];
        for q in 0..k {
            for a in 0..2 {
                let r = q * 2 + a;
                trits[r * or_w + q] = 1; // state_q
                // a=1 接 +sym，a=0 接 −sym。
                trits[r * or_w + k + 2 * step] = if a == 1 { 1 } else { -1 };
                bias[r] = -1;
            }
        }
        for i in 0..dim_in {
            trits[(2 * k + i) * or_w + k + i] = 1;
        }
        for j in 0..c {
            bias[2 * k + dim_in + j] = 1;
        }
        layers.push(Layer::new(and_w, or_w, &trits, &bias).map_err(|e| e.to_string())?);

        // OR 层：k 个新状态 + dim_in 个输入传递 + c 个常量。
        let mut trits = vec![0i8; or_w * and_w];
        let mut bias = vec![0i8; or_w];
        let const_base = 2 * k + dim_in;
        for (j, src) in sources.iter().enumerate() {
            match src.len() {
                0 => bias[j] = -1,
                _ => {
                    for &s in src {
                        trits[j * and_w + s] = 1;
                    }
                    // n−1 个常量，把阈值推到「任意一个 +1 即接受」。
                    for t in 0..src.len() - 1 {
                        trits[j * and_w + const_base + t] = 1;
                    }
                }
            }
        }
        for i in 0..dim_in {
            trits[(k + i) * and_w + 2 * k + i] = 1;
        }
        for j in 0..c {
            bias[k + dim_in + j] = 1;
        }
        layers.push(Layer::new(or_w, and_w, &trits, &bias).map_err(|e| e.to_string())?);
    }

    // 输出层：接受状态的 OR（同样用常量补阈值）。
    let mut trits = vec![0i8; or_w];
    let mut bias = vec![0i8; 1];
    match accepting.len() {
        0 => bias[0] = -1,
        _ => {
            for &j in &accepting {
                trits[j] = 1;
            }
            for t in 0..accepting.len() - 1 {
                trits[k + dim_in + t] = 1;
            }
        }
    }
    layers.push(Layer::new(1, or_w, &trits, &bias).map_err(|e| e.to_string())?);

    Net::new(layers).map_err(|e| e.to_string())
}

/// 用编译出的网络在定长域上判决一个串。
#[must_use]
pub fn run(net: &Net, w: &[Trit], max_len: usize) -> bool {
    let input = encode_sym(w, max_len);
    net.forward(&input)
        .map(|out| out[0] == Trit::Pos)
        .unwrap_or(false)
}
