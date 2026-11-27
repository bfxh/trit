use std::collections::HashMap;

use crate::direct::ir::Op;
use crate::direct::lower::prune_dead;

use super::assoc;
use super::key::{Key, key_of, normalize, remap_op};
use super::xorchain;

pub(super) fn pass(ops: Vec<Op>) -> Vec<Op> {
    let root = ops.len() - 1;
    let mut remap = vec![usize::MAX; ops.len()];
    let mut kept = Vec::with_capacity(ops.len());
    let mut memo: HashMap<Key, usize> = HashMap::new();
    let mut chains: HashMap<assoc::Key, usize> = HashMap::new();
    let mut xors: HashMap<xorchain::Key, usize> = HashMap::new();
    for (index, op) in ops.into_iter().enumerate() {
        let op = normalize(remap_op(op, &remap));
        if let Op::And(left, right) | Op::Or(left, right) = op {
            let dual = matches!(op, Op::Or(..));
            remap[index] = assoc::norm(&mut kept, &mut chains, dual, left, right);
            continue;
        }
        if let Op::Xor2(left, right) = op {
            remap[index] = xorchain::norm(&mut kept, &mut xors, left, right);
            continue;
        }
        let key = key_of(op);
        let out = match memo.get(&key) {
            Some(&out) => out,
            None => {
                let out = kept.len();
                kept.push(op);
                memo.insert(key, out);
                out
            }
        };
        remap[index] = out;
    }
    prune_dead(kept, remap[root])
}
