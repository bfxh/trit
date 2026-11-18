//! 轨迹层：抓一次前向的中间状态，并与提取的 DFA 状态对齐。
//!
//! 回答设计文档 L-D 的第一半「中间状态序列是什么」：
//!
//! - `hidden` = 层 1 输出的三值激活向量（本网络 9 个单元）；
//! - 对齐读数回答两个不同的问题：
//!   1. 同一 `hidden` 是否决定**输出**（信息瓶颈）——必须成立；
//!   2. 同一 `hidden` 是否决定 **DFA 状态**——预期**不**成立：域内最小 DFA 的状态含
//!      「剩余可扩展长度」，而网络内部并不表示长度。这条读数本身就是「域截断把信息
//!      编进了状态」的证据。

use std::collections::{HashMap, HashSet};

use trit_wspace::{Net, Trit};

use crate::dfa::Dfa;
use crate::word;

/// 一条轨迹：串 + 层 1 激活 + 输出层 pre-activation + 最终输出。
#[derive(Clone, Debug)]
pub struct Trace {
    pub word: Vec<Trit>,
    pub hidden: Vec<Trit>,
    pub pre_out: i32,
    pub out: Trit,
}

/// 取一条轨迹。
#[must_use]
pub fn trace(net: &Net, w: &[Trit], max_len: usize) -> Trace {
    let input = word::encode(w, max_len);
    let pre = net.pre_activations(&input).expect("编码后维度一致");
    let hidden: Vec<Trit> = pre[0].iter().map(|v| Trit::sign_of(*v)).collect();
    let pre_out = pre[1][0];
    Trace {
        word: w.to_vec(),
        hidden,
        pre_out,
        out: Trit::sign_of(pre_out),
    }
}

/// 状态对齐读数。
#[derive(Clone, Copy, Debug)]
pub struct Align {
    /// 域内出现过的不同 `hidden` 向量数。
    pub hidden_classes: usize,
    /// 同一 `hidden` 对应多个输出的冲突数（**必须 0**）。
    pub hidden_to_out_conflicts: usize,
    /// 同一 `hidden` 对应多个 DFA 状态的冲突数（预期 > 0，见模块文档）。
    pub hidden_to_state_conflicts: usize,
    /// 同一 DFA 状态最多对应几种 `hidden`。
    pub max_variants: usize,
}

/// 在声明域上做对齐统计。
#[must_use]
pub fn align(net: &Net, max_len: usize, dfa: &Dfa) -> Align {
    let words = word::all_words(max_len);
    let mut hidden_out: HashMap<Vec<Trit>, HashSet<Trit>> = HashMap::new();
    let mut hidden_state: HashMap<Vec<Trit>, HashSet<usize>> = HashMap::new();
    let mut state_hidden: HashMap<usize, HashSet<Vec<Trit>>> = HashMap::new();
    for w in &words {
        let t = trace(net, w, max_len);
        let bits: Vec<bool> = w.iter().map(|x| *x == Trit::Pos).collect();
        let s = dfa.state_of(&bits);
        hidden_out
            .entry(t.hidden.clone())
            .or_default()
            .insert(t.out);
        hidden_state.entry(t.hidden.clone()).or_default().insert(s);
        state_hidden.entry(s).or_default().insert(t.hidden);
    }
    Align {
        hidden_classes: hidden_out.len(),
        hidden_to_out_conflicts: hidden_out.values().filter(|v| v.len() > 1).count(),
        hidden_to_state_conflicts: hidden_state.values().filter(|v| v.len() > 1).count(),
        max_variants: state_hidden.values().map(|v| v.len()).max().unwrap_or(0),
    }
}
