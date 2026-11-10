//! 节点三态位集：稠密图与 CSR 共用同一套节点存储。

use trit_core::value::Ternary;

#[derive(Clone, Debug)]
pub(crate) struct NodeStates {
    n: usize,
    words: usize,
    pos: Vec<u64>,
    neg: Vec<u64>,
    critical: Vec<u64>,
}

impl NodeStates {
    pub fn new(n: usize) -> Self {
        let words = n.div_ceil(64);
        Self {
            n,
            words,
            pos: vec![0; words],
            neg: vec![0; words],
            critical: vec![0; words],
        }
    }

    pub fn len(&self) -> usize {
        self.n
    }

    pub fn words(&self) -> usize {
        self.words
    }

    pub fn set(&mut self, node: usize, value: Ternary) {
        assert!(node < self.n, "节点越界：{node} / {}", self.n);
        let (word, bit) = (node / 64, 1u64 << (node % 64));
        match value {
            Ternary::True => {
                self.pos[word] |= bit;
                self.neg[word] &= !bit;
            }
            Ternary::False => {
                self.neg[word] |= bit;
                self.pos[word] &= !bit;
            }
            Ternary::Unknown => {
                self.pos[word] &= !bit;
                self.neg[word] &= !bit;
            }
        }
    }

    pub fn get(&self, node: usize) -> Ternary {
        assert!(node < self.n, "节点越界：{node} / {}", self.n);
        let (word, bit) = (node / 64, 1u64 << (node % 64));
        if self.pos[word] & bit != 0 {
            Ternary::True
        } else if self.neg[word] & bit != 0 {
            Ternary::False
        } else {
            Ternary::Unknown
        }
    }

    pub fn set_critical(&mut self, node: usize, critical: bool) {
        assert!(node < self.n, "节点越界：{node} / {}", self.n);
        let (word, bit) = (node / 64, 1u64 << (node % 64));
        if critical {
            self.critical[word] |= bit;
        } else {
            self.critical[word] &= !bit;
        }
    }

    pub fn is_critical(&self, node: usize) -> bool {
        assert!(node < self.n, "节点越界：{node} / {}", self.n);
        self.critical[node / 64] & (1u64 << (node % 64)) != 0
    }
}
