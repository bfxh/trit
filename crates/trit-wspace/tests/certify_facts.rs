//! `patch_facts` 的验证性增强：tile / 行级编辑的 noop 与 repeated 必须与
//! “应用后网络是否逐位变化”的暴力判定一致。

use std::collections::HashMap;

use trit_wspace::certify::patch_facts;
use trit_wspace::{Edit, Layer, Net, Patch, Trit};

fn base_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("net")
}

fn zero_first_row_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[0, 0, -1, -1], &[0, -1]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("net")
}

#[test]
fn negate_row_noop_only_when_row_is_all_zero() {
    let net = zero_first_row_net();
    let noop = Patch::new(vec![Edit::NegateRow { layer: 0, row: 0 }]);
    assert_eq!(patch_facts(&net, &noop).noop, 1);
    let mut applied = net.clone();
    noop.apply(&mut applied).expect("apply");
    assert_eq!(applied, net);

    let changed = Patch::new(vec![Edit::NegateRow { layer: 0, row: 1 }]);
    assert_eq!(patch_facts(&net, &changed).noop, 0);
}

#[test]
fn set_tile_and_fill_tile_noop_are_detected() {
    let net = zero_first_row_net();
    // 第一行 [0, 0] 用 0 填充：等于当前值。
    let fill = Patch::new(vec![Edit::FillTile {
        layer: 0,
        row_start: 0,
        rows: 1,
        cols: 2,
        col_start: 0,
        to: Trit::Zero,
    }]);
    assert_eq!(patch_facts(&net, &fill).noop, 1);
    let mut applied = net.clone();
    fill.apply(&mut applied).expect("apply");
    assert_eq!(applied, net);

    // 整块 2×2 逐位等于当前值：noop。
    let tile = Patch::new(vec![Edit::SetTile {
        layer: 0,
        row_start: 0,
        rows: 2,
        cols: 2,
        col_start: 0,
        values: vec![Trit::Zero, Trit::Zero, Trit::Neg, Trit::Neg],
    }]);
    assert_eq!(patch_facts(&net, &tile).noop, 1);

    // 改一格就不是 noop。
    let changed = Patch::new(vec![Edit::SetTile {
        layer: 0,
        row_start: 0,
        rows: 2,
        cols: 2,
        col_start: 0,
        values: vec![Trit::Pos, Trit::Zero, Trit::Neg, Trit::Neg],
    }]);
    assert_eq!(patch_facts(&net, &changed).noop, 0);
}

#[test]
fn tile_coordinates_participate_in_repeated() {
    let net = zero_first_row_net();
    let patch = Patch::new(vec![
        Edit::FillTile {
            layer: 0,
            row_start: 0,
            rows: 1,
            cols: 2,
            col_start: 0,
            to: Trit::Zero,
        },
        Edit::SetTrit {
            layer: 0,
            row: 0,
            col: 0,
            to: Trit::Pos,
        },
    ]);
    let facts = patch_facts(&net, &patch);
    // (0,0) 被 tile 写一次、SetTrit 再写一次 ⇒ 1 条重复；(0,1) 只写一次。
    assert_eq!(facts.repeated, 1);
    assert_eq!(facts.noop, 1);
}

#[test]
fn swap_rows_noop_when_rows_are_identical() {
    let net = Net::new(vec![
        Layer::new(2, 2, &[0, 0, 0, 0], &[0, 0]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("net");
    let patch = Patch::new(vec![Edit::SwapRows {
        layer: 0,
        a: 0,
        b: 1,
    }]);
    assert_eq!(patch_facts(&net, &patch).noop, 1);
    let mut applied = net.clone();
    patch.apply(&mut applied).expect("apply");
    assert_eq!(applied, net);
}

#[test]
fn edge_tiles_do_not_panic_or_count_as_noop() {
    let net = base_net();
    for patch in [
        Patch::new(vec![Edit::SetTile {
            layer: 0,
            row_start: 1,
            rows: 2,
            cols: 2,
            col_start: 0,
            values: vec![Trit::Zero; 4],
        }]),
        Patch::new(vec![Edit::SetTile {
            layer: 0,
            row_start: 0,
            rows: 1,
            cols: 2,
            col_start: 0,
            values: vec![Trit::Zero; 3],
        }]),
        Patch::new(vec![Edit::FillTile {
            layer: 9,
            row_start: 0,
            rows: 1,
            cols: 1,
            col_start: 0,
            to: Trit::Zero,
        }]),
    ] {
        assert_eq!(patch_facts(&net, &patch).noop, 0);
    }
}

// ── 暴力对拍 ────────────────────────────────────────────────────────────

#[test]
fn random_patches_match_bruteforce() {
    for seed in 1..=60u64 {
        let mut rng = Rng(seed);
        let net = base_net();
        let patch = random_patch(&mut rng, &net);
        let facts = patch_facts(&net, &patch);

        let brute_noop = patch
            .edits()
            .iter()
            .filter(|edit| brute_noop(&net, edit))
            .count();
        assert_eq!(facts.noop, brute_noop, "seed={seed}");

        let brute_repeated = brute_repeated(&net, &patch);
        assert_eq!(facts.repeated, brute_repeated, "seed={seed}");

        assert_eq!(facts.touched, patch.cost(&net).touched, "seed={seed}");
    }
}

fn brute_noop(net: &Net, edit: &Edit) -> bool {
    let mut copy = net.clone();
    if Patch::new(vec![edit.clone()]).apply(&mut copy).is_err() {
        return false;
    }
    copy == *net
}

fn brute_repeated(net: &Net, patch: &Patch) -> usize {
    let mut seen: HashMap<(usize, u8, usize, usize), usize> = HashMap::new();
    let mut repeated = 0;
    for edit in patch.edits() {
        for key in coordinates(net, edit) {
            let count = seen.entry(key).or_insert(0);
            if *count > 0 {
                repeated += 1;
            }
            *count += 1;
        }
    }
    repeated
}

fn coordinates(net: &Net, edit: &Edit) -> Vec<(usize, u8, usize, usize)> {
    match *edit {
        Edit::SetTrit {
            layer, row, col, ..
        } => vec![(layer, 0, row, col)],
        Edit::SetBias { layer, row, .. } => vec![(layer, 1, row, 0)],
        Edit::SetScale { layer, row, .. } => vec![(layer, 2, row, 0)],
        Edit::SwapRows { .. } => Vec::new(),
        Edit::NegateRow { layer, row } => {
            let Some(one) = net.layer(layer) else {
                return Vec::new();
            };
            if row >= one.rows() {
                return Vec::new();
            }
            let mut out: Vec<_> = (0..one.cols()).map(|col| (layer, 0, row, col)).collect();
            out.push((layer, 1, row, 0));
            out
        }
        Edit::SetTile {
            layer,
            row_start,
            rows,
            cols,
            col_start,
            ..
        }
        | Edit::FillTile {
            layer,
            row_start,
            rows,
            cols,
            col_start,
            ..
        } => {
            let mut out = Vec::with_capacity(rows * cols);
            for r in 0..rows {
                for c in 0..cols {
                    out.push((layer, 0, row_start + r, col_start + c));
                }
            }
            out
        }
    }
}

fn random_patch(rng: &mut Rng, net: &Net) -> Patch {
    let mut edits = Vec::new();
    for _ in 0..4 {
        let layer = rng.pick(net.layers().len());
        let one = net.layer(layer).expect("layer");
        let rows = one.rows();
        let cols = one.cols();
        let row = rng.pick(rows);
        let col = rng.pick(cols);
        match rng.pick(7) {
            0 => edits.push(Edit::SetTrit {
                layer,
                row,
                col,
                to: Trit::ALL[rng.pick(3)],
            }),
            1 => edits.push(Edit::SetBias {
                layer,
                row,
                to: Trit::ALL[rng.pick(3)],
            }),
            2 => edits.push(Edit::SetScale {
                layer,
                row,
                to: 1.0,
            }),
            3 => edits.push(Edit::SwapRows {
                layer,
                a: row,
                b: rng.pick(rows),
            }),
            4 => edits.push(Edit::NegateRow { layer, row }),
            5 => {
                let rows = 1 + rng.pick(rows - row);
                let cols = 1 + rng.pick(cols - col);
                let values = (0..rows * cols).map(|_| Trit::ALL[rng.pick(3)]).collect();
                edits.push(Edit::SetTile {
                    layer,
                    row_start: row,
                    rows,
                    cols,
                    col_start: col,
                    values,
                });
            }
            _ => {
                let rows = 1 + rng.pick(rows - row);
                let cols = 1 + rng.pick(cols - col);
                edits.push(Edit::FillTile {
                    layer,
                    row_start: row,
                    rows,
                    cols,
                    col_start: col,
                    to: Trit::ALL[rng.pick(3)],
                });
            }
        }
    }
    Patch::new(edits)
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn pick(&mut self, n: usize) -> usize {
        (self.next() as usize) % n
    }
}
