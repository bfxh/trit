//! M0f：DFA → 三值网络 的编译器（Tracr 反向闭环的前半）。
//!
//! 展开 `max_len` 步，每步 2 层：
//!
//! ```text
//! AND 层：对每个 (状态 q, 符号 a) 算 sign(state_q + sym_a − 1)
//! OR  层：new_state_j = OR_{δ(q,a)=j} 该项
//! ```
//!
//! **三个符号**：`0`、`1`、`End`（串结束后的填充）。输入每位置 3 维 one-hot，
//! `End` 的转移一律自环（`δ(q, End) = q`），于是「读完就冻结」不需要 running 标志，
//! 编译器也因此支持变长串。
//!
//! **OR 的任意扇入**：`OR(x1..xn) = sign(Σxi + (n−1))`，而 `n−1` 由 `n−1` 个恒为
//! `Pos` 的常量单元供给（权重只能 ±1、bias 只能 ±1/0，但常量单元不受限）。
//! 因此每步仍是 2 层，不需要树形归约。

use trit_wspace::{Layer, Net, Trit};

use crate::dfa::Dfa;

/// 编译器用的符号编码：每位置 3 维 one-hot（0 / 1 / End）。
#[must_use]
pub fn encode3(w: &[Trit], max_len: usize) -> Vec<Trit> {
    let mut out = Vec::with_capacity(max_len * 3);
    for i in 0..max_len {
        match w.get(i) {
            Some(Trit::Pos) => {
                out.push(Trit::Neg);
                out.push(Trit::Pos);
                out.push(Trit::Neg);
            }
            Some(Trit::Neg) => {
                out.push(Trit::Pos);
                out.push(Trit::Neg);
                out.push(Trit::Neg);
            }
            _ => {
                out.push(Trit::Neg);
                out.push(Trit::Neg);
                out.push(Trit::Pos);
            }
        }
    }
    out
}

/// 把 DFA 编译成展开 `max_len` 步的三值前馈网络（字母表扩展为 `{0, 1, End}`）。
pub fn compile(dfa: &Dfa, max_len: usize) -> Result<Net, String> {
    let k = dfa.states;
    let dim_in = max_len * 3;

    // 扩成 3 符号：End 一律自环。
    let delta3: Vec<[usize; 3]> = (0..k)
        .map(|q| [dfa.delta[q][0], dfa.delta[q][1], q])
        .collect();

    // 每个目标状态的转移源（用 `q*3 + a` 编码）。
    let sources: Vec<Vec<usize>> = (0..k)
        .map(|j| {
            (0..k)
                .flat_map(|q| (0..3).map(move |a| (q, a)))
                .filter(|(q, a)| delta3[*q][*a] == j)
                .map(|(q, a)| q * 3 + a)
                .collect()
        })
        .collect();
    let accepting: Vec<usize> = (0..k).filter(|j| dfa.accept[*j]).collect();

    // 常量单元数：`max(源数, 接受数) − 1`。
    let max_src = sources.iter().map(Vec::len).max().unwrap_or(0);
    let c = max_src.max(accepting.len()).saturating_sub(1);

    let or_w = k + dim_in + c;
    let and_w = 3 * k + dim_in + c;

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
        // AND 层：3k 个 AND 项 + dim_in 个输入传递 + c 个常量。
        let mut trits = vec![0i8; and_w * or_w];
        let mut bias = vec![0i8; and_w];
        for q in 0..k {
            for a in 0..3 {
                let r = q * 3 + a;
                trits[r * or_w + q] = 1; // state_q
                trits[r * or_w + k + 3 * step + a] = 1; // 第 step 步的 one-hot 位
                bias[r] = -1;
            }
        }
        for i in 0..dim_in {
            trits[(3 * k + i) * or_w + k + i] = 1;
        }
        for j in 0..c {
            bias[3 * k + dim_in + j] = 1;
        }
        layers.push(Layer::new(and_w, or_w, &trits, &bias).map_err(|e| e.to_string())?);

        // OR 层：k 个新状态 + dim_in 个输入传递 + c 个常量。
        let mut trits = vec![0i8; or_w * and_w];
        let mut bias = vec![0i8; or_w];
        let const_base = 3 * k + dim_in;
        for (j, src) in sources.iter().enumerate() {
            match src.len() {
                0 => bias[j] = -1,
                _ => {
                    for &s in src {
                        trits[j * and_w + s] = 1;
                    }
                    for t in 0..src.len() - 1 {
                        trits[j * and_w + const_base + t] = 1;
                    }
                }
            }
        }
        for i in 0..dim_in {
            trits[(k + i) * and_w + 3 * k + i] = 1;
        }
        for j in 0..c {
            bias[k + dim_in + j] = 1;
        }
        layers.push(Layer::new(or_w, and_w, &trits, &bias).map_err(|e| e.to_string())?);
    }

    // 输出层：接受状态的 OR。
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

/// 用编译出的网络在变长域上判决一个串。
#[must_use]
pub fn run(net: &Net, w: &[Trit], max_len: usize) -> bool {
    let input = encode3(w, max_len);
    net.forward(&input)
        .map(|out| out[0] == Trit::Pos)
        .unwrap_or(false)
}

/// 初始状态 one-hot（`start = 0`）。
#[must_use]
pub fn initial_state(k: usize) -> Vec<Trit> {
    (0..k)
        .map(|i| if i == 0 { Trit::Pos } else { Trit::Neg })
        .collect()
}

/// 把 DFA 编译成**单步 cell**：输入 `[state(k), sym(3)]`，输出 `new_state(k)`。
///
/// 与 [`compile`] 的区别：这里只编一步，状态由外部循环携带——同一权重重复用，
/// 所以轨迹是真正的串行执行序列（M3）。只支持每目标状态 ≤ 2 个转移源：
/// OR 层没有空间放常量单元（输出维度就是 `k`）。
pub fn compile_cell(dfa: &Dfa) -> Result<Net, String> {
    let k = dfa.states;
    let in_dim = k + 3;

    let delta3: Vec<[usize; 3]> = (0..k)
        .map(|q| [dfa.delta[q][0], dfa.delta[q][1], q])
        .collect();
    let sources: Vec<Vec<usize>> = (0..k)
        .map(|j| {
            (0..k)
                .flat_map(|q| (0..3).map(move |a| (q, a)))
                .filter(|(q, a)| delta3[*q][*a] == j)
                .map(|(q, a)| q * 3 + a)
                .collect()
        })
        .collect();
    for (j, src) in sources.iter().enumerate() {
        if src.len() > 2 {
            return Err(format!("状态 {j} 有 {} 个转移源，cell 版只支持 ≤ 2", src.len()));
        }
    }

    // AND 层：3k 行（每个 (q, a) 一项），cols = in_dim。
    let mut l1 = vec![0i8; 3 * k * in_dim];
    let mut b1 = vec![0i8; 3 * k];
    for q in 0..k {
        for a in 0..3 {
            let r = q * 3 + a;
            l1[r * in_dim + q] = 1;
            l1[r * in_dim + k + a] = 1;
            b1[r] = -1;
        }
    }
    let layer1 = Layer::new(3 * k, in_dim, &l1, &b1).map_err(|e| e.to_string())?;

    // OR 层：k 行，cols = 3k。
    let mut l2 = vec![0i8; k * 3 * k];
    let mut b2 = vec![0i8; k];
    for (j, src) in sources.iter().enumerate() {
        match src.len() {
            0 => b2[j] = -1,
            1 => l2[j * 3 * k + src[0]] = 1,
            _ => {
                for &s in src {
                    l2[j * 3 * k + s] = 1;
                }
                b2[j] = 1;
            }
        }
    }
    let layer2 = Layer::new(k, 3 * k, &l2, &b2).map_err(|e| e.to_string())?;

    Net::new(vec![layer1, layer2]).map_err(|e| e.to_string())
}
