//! M0f：DFA → 三值网络 的编译器（Tracr 反向闭环的前半）。
//!
//! 展开 `max_len` 步，每步 2 层：
//!
//! ```text
//! AND 层：对每个 (状态 q, 符号 a) 算 sign(state_q + (±1)·sym_t − 1)
//! OR  层：new_state_j = OR_{δ(q,a)=j} 该项
//! ```
//!
//! `a=1` 时接 `+sym`、`a=0` 时接 `−sym`；状态 one-hot 取 `{Neg, Pos}`。
//! 输入每位置 2 维 `(sym, is_end)`：`0 → (Neg, Neg)`、`1 → (Pos, Neg)`、填充 → `(Zero, Pos)`。
//! 注意这里**不用** `word::encode`——那一版把 0 编成 `Zero`（为线性阈值忽略填充），
//! 而阈值门需要 `{±1}`，两个编码不能混用。
//!
//! 支持范围（超出即报错，不静默降级）：
//! - 每个目标状态最多 2 个转移源；
//! - 接受状态最多 2 个；
//! - **只处理定长串**：不实现 End 冻结（未证栏）。

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
    let dim = k + dim_in;

    for j in 0..k {
        let sources = (0..k)
            .flat_map(|q| (0..2).map(move |a| (q, a)))
            .filter(|(q, a)| dfa.delta[*q][*a] == j)
            .count();
        if sources > 2 {
            return Err(format!("状态 {j} 有 {sources} 个转移源，超过编译器支持的 2 个"));
        }
    }
    let accepting: Vec<usize> = (0..k).filter(|j| dfa.accept[*j]).collect();
    if accepting.len() > 2 {
        return Err(format!(
            "接受状态有 {} 个，超过编译器支持的 2 个",
            accepting.len()
        ));
    }

    let mut layers: Vec<Layer> = Vec::new();

    // 层 0：初始化状态（bias 给出 one-hot）+ 传递输入。
    let mut trits = vec![0i8; dim * dim_in];
    let mut bias = vec![0i8; dim];
    for i in 0..k {
        bias[i] = if i == dfa.start { 1 } else { -1 };
    }
    for i in 0..dim_in {
        trits[(k + i) * dim_in + i] = 1;
    }
    layers.push(Layer::new(dim, dim_in, &trits, &bias).map_err(|e| e.to_string())?);

    for step in 0..max_len {
        // AND 层：k*2 个 AND 项 + dim_in 个输入传递。
        let and_rows = k * 2 + dim_in;
        let mut trits = vec![0i8; and_rows * dim];
        let mut bias = vec![0i8; and_rows];
        for q in 0..k {
            for a in 0..2 {
                let r = q * 2 + a;
                trits[r * dim + q] = 1; // state_q
                // a=1 接 +sym，a=0 接 −sym。
                trits[r * dim + k + 2 * step] = if a == 1 { 1 } else { -1 };
                bias[r] = -1;
            }
        }
        for i in 0..dim_in {
            trits[(k * 2 + i) * dim + k + i] = 1;
        }
        layers.push(Layer::new(and_rows, dim, &trits, &bias).map_err(|e| e.to_string())?);

        // OR 层：k 个新状态 + dim_in 个输入传递。
        let or_rows = k + dim_in;
        let mut trits = vec![0i8; or_rows * and_rows];
        let mut bias = vec![0i8; or_rows];
        for j in 0..k {
            let sources: Vec<usize> = (0..k)
                .flat_map(|q| (0..2).map(move |a| (q, a)))
                .filter(|(q, a)| dfa.delta[*q][*a] == j)
                .map(|(q, a)| q * 2 + a)
                .collect();
            match sources.len() {
                0 => bias[j] = -1,
                1 => trits[j * and_rows + sources[0]] = 1,
                _ => {
                    for s in sources {
                        trits[j * and_rows + s] = 1;
                    }
                    bias[j] = 1;
                }
            }
        }
        for i in 0..dim_in {
            trits[(k + i) * and_rows + k * 2 + i] = 1;
        }
        layers.push(Layer::new(or_rows, and_rows, &trits, &bias).map_err(|e| e.to_string())?);
    }

    // 输出层：接受状态的 OR。
    let last = k + dim_in;
    let mut trits = vec![0i8; last];
    let mut bias = vec![0i8; 1];
    match accepting.len() {
        0 => bias[0] = -1,
        1 => trits[accepting[0]] = 1,
        _ => {
            for j in accepting {
                trits[j] = 1;
            }
            bias[0] = 1;
        }
    }
    layers.push(Layer::new(1, last, &trits, &bias).map_err(|e| e.to_string())?);

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
