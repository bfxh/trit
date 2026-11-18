//! DFA、Moore 最小化，以及前缀树构造（网络 / 语言定义两条入口）。

use std::collections::{HashMap, VecDeque};

use trit_wspace::Trit;

use crate::word;

/// 二字母表 DFA（`delta[s][0]` 走 0，`delta[s][1]` 走 1）。
#[derive(Clone, Debug)]
pub struct Dfa {
    pub states: usize,
    pub start: usize,
    pub accept: Vec<bool>,
    pub delta: Vec<[usize; 2]>,
}

impl Dfa {
    /// 走一遍串，返回是否接受。
    #[must_use]
    pub fn run(&self, input: &[bool]) -> bool {
        let mut s = self.start;
        for &b in input {
            s = self.delta[s][usize::from(b)];
        }
        self.accept[s]
    }

    /// Moore 划分细化，得到最小 DFA（再剔除不可达状态、从 `start` 重编号）。
    #[must_use]
    pub fn minimize(&self) -> Self {
        let mut block: Vec<usize> = self.accept.iter().map(|a| usize::from(*a)).collect();
        loop {
            let key: Vec<(usize, usize, usize)> = (0..self.states)
                .map(|s| {
                    (
                        block[s],
                        block[self.delta[s][0]],
                        block[self.delta[s][1]],
                    )
                })
                .collect();
            let next = renumber(&key);
            if next == block {
                break;
            }
            block = next;
        }
        let n = block.iter().copied().max().map_or(0, |m| m + 1);
        let mut rep = vec![0usize; n];
        for (s, &b) in block.iter().enumerate() {
            rep[b] = s;
        }
        let accept = (0..n).map(|b| self.accept[rep[b]]).collect();
        let delta = (0..n)
            .map(|b| [block[self.delta[rep[b]][0]], block[self.delta[rep[b]][1]]])
            .collect();
        Self {
            states: n,
            start: block[self.start],
            accept,
            delta,
        }
        .canonicalize()
    }

    /// 只保留从 `start` 可达的状态，并按 BFS 顺序重编号。
    #[must_use]
    pub fn canonicalize(self) -> Self {
        let mut order = vec![usize::MAX; self.states];
        let mut queue = VecDeque::new();
        order[self.start] = 0;
        queue.push_back(self.start);
        let mut n = 1usize;
        while let Some(s) = queue.pop_front() {
            for a in 0..2 {
                let t = self.delta[s][a];
                if order[t] == usize::MAX {
                    order[t] = n;
                    n += 1;
                    queue.push_back(t);
                }
            }
        }
        let mut inv = vec![0usize; n];
        for (s, &o) in order.iter().enumerate() {
            if o != usize::MAX {
                inv[o] = s;
            }
        }
        let accept = (0..n).map(|b| self.accept[inv[b]]).collect();
        let delta = (0..n)
            .map(|b| {
                [
                    order[self.delta[inv[b]][0]],
                    order[self.delta[inv[b]][1]],
                ]
            })
            .collect();
        Self {
            states: n,
            start: 0,
            accept,
            delta,
        }
    }
}

/// 按 `key` 给状态重新编号：相同 key 同块。
fn renumber<K: Ord>(key: &[K]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..key.len()).collect();
    order.sort_by(|&a, &b| key[a].cmp(&key[b]));
    let mut out = vec![0usize; key.len()];
    let mut cur = 0usize;
    out[order[0]] = 0;
    for w in order.windows(2) {
        if key[w[0]] != key[w[1]] {
            cur += 1;
        }
        out[w[1]] = cur;
    }
    out
}

/// 定长 `len` 的**前缀树**自动机（未最小化）：
///
/// - 状态 = 所有长度 `0..=len` 的前缀，外加一个 `DEAD`（越界后自环）；
/// - 转移 = 加一位；长度到 `len` 后走 `DEAD`；
/// - 接受性由 `accepts` 决定，`DEAD` 一律不接受。
///
/// 这条入口同时服务两条对拍路线：网络判决、语言定义。
#[must_use]
pub fn from_accepts(len: usize, accepts: impl Fn(&[Trit]) -> bool) -> Dfa {
    let pre = word::all_words(len);
    let dead = pre.len();
    let index: HashMap<&[Trit], usize> = pre
        .iter()
        .enumerate()
        .map(|(i, w)| (w.as_slice(), i))
        .collect();
    let mut delta = vec![[dead; 2]; dead + 1];
    for (i, p) in pre.iter().enumerate() {
        if p.len() < len {
            for a in 0..2 {
                let mut q = p.clone();
                q.push(word::symbol(a == 1));
                delta[i][a] = index[q.as_slice()];
            }
        }
    }
    let mut accept: Vec<bool> = pre.iter().map(|p| accepts(p)).collect();
    accept.push(false);
    Dfa {
        states: dead + 1,
        start: 0,
        accept,
        delta,
    }
}
