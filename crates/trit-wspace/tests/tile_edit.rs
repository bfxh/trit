//! tile 编辑：`SetTile` / `FillTile` 的事务、回滚、JSON 与审计重放。

use serde_json::json;
use trit_wspace::wire;
use trit_wspace::{AuditLog, Edit, Layer, Net, Patch, Trit};

fn sample() -> Net {
    Net::new(vec![
        Layer::with_scales(
            3,
            4,
            &[1, 0, -1, 1, 0, 1, 0, -1, -1, 0, 1, 0],
            &[1, -1, 0],
            vec![0.5, 1.0, 1.5],
        )
        .expect("layer0"),
        Layer::new(2, 3, &[1, -1, 0, 0, 1, -1], &[0, 1]).expect("layer1"),
    ])
    .expect("net")
}

fn trit(net: &Net, layer: usize, row: usize, col: usize) -> Trit {
    net.layer(layer)
        .and_then(|target| target.trit(row, col))
        .expect("trit")
}

#[test]
fn set_tile_writes_only_the_rectangle() {
    let mut net = sample();
    let original = net.clone();
    let patch = Patch::new(vec![Edit::SetTile {
        layer: 0,
        row_start: 1,
        rows: 2,
        col_start: 1,
        cols: 2,
        values: vec![Trit::Pos, Trit::Neg, Trit::Zero, Trit::Pos],
    }]);
    let journal = patch.apply(&mut net).expect("apply");
    assert_eq!(trit(&net, 0, 1, 1), Trit::Pos);
    assert_eq!(trit(&net, 0, 1, 2), Trit::Neg);
    assert_eq!(trit(&net, 0, 2, 1), Trit::Zero);
    assert_eq!(trit(&net, 0, 2, 2), Trit::Pos);
    assert_eq!(trit(&net, 0, 0, 0), trit(&original, 0, 0, 0));
    assert_eq!(trit(&net, 0, 1, 0), trit(&original, 0, 1, 0));
    assert_eq!(trit(&net, 0, 1, 3), trit(&original, 0, 1, 3));
    assert_eq!(trit(&net, 1, 0, 0), trit(&original, 1, 0, 0));

    journal.undo(&mut net);
    assert_eq!(net, original);
}

#[test]
fn fill_tile_and_multi_edit_roll_back_exactly() {
    let mut net = sample();
    let original = net.clone();
    let patch = Patch::new(vec![
        Edit::FillTile {
            layer: 1,
            row_start: 0,
            rows: 2,
            col_start: 1,
            cols: 2,
            to: Trit::Neg,
        },
        Edit::SetTile {
            layer: 0,
            row_start: 0,
            rows: 1,
            col_start: 2,
            cols: 2,
            values: vec![Trit::Zero, Trit::Pos],
        },
    ]);
    let journal = patch.apply(&mut net).expect("apply");
    for row in 0..2 {
        for col in 1..3 {
            assert_eq!(trit(&net, 1, row, col), Trit::Neg);
        }
    }
    assert_eq!(trit(&net, 0, 0, 2), Trit::Zero);
    assert_eq!(trit(&net, 0, 0, 3), Trit::Pos);

    journal.undo(&mut net);
    assert_eq!(net, original);
}

#[test]
fn rejects_bad_tiles_without_touching_the_net() {
    let mut net = sample();
    let original = net.clone();
    let bad = [
        Edit::FillTile {
            layer: 0,
            row_start: 2,
            rows: 2,
            col_start: 0,
            cols: 1,
            to: Trit::Pos,
        },
        Edit::FillTile {
            layer: 0,
            row_start: 0,
            rows: 1,
            col_start: 3,
            cols: 2,
            to: Trit::Pos,
        },
        Edit::FillTile {
            layer: 0,
            row_start: 0,
            rows: 0,
            col_start: 0,
            cols: 1,
            to: Trit::Pos,
        },
        Edit::SetTile {
            layer: 0,
            row_start: 0,
            rows: 1,
            col_start: 0,
            cols: 2,
            values: vec![Trit::Pos],
        },
        Edit::FillTile {
            layer: 9,
            row_start: 0,
            rows: 1,
            col_start: 0,
            cols: 1,
            to: Trit::Pos,
        },
    ];
    for edit in bad {
        assert!(Patch::new(vec![edit]).apply(&mut net).is_err());
        assert_eq!(net, original);
    }
}

#[test]
fn tile_json_round_trips() {
    let edit = Edit::SetTile {
        layer: 0,
        row_start: 1,
        rows: 1,
        col_start: 2,
        cols: 3,
        values: vec![Trit::Pos, Trit::Neg, Trit::Zero],
    };
    let value = wire::edit_to_value(&edit);
    assert_eq!(wire::edit_from_value(&value).expect("edit"), edit);

    let patch = Patch::new(vec![
        edit,
        Edit::FillTile {
            layer: 1,
            row_start: 0,
            rows: 2,
            col_start: 1,
            cols: 1,
            to: Trit::Neg,
        },
    ]);
    let value = wire::patch_to_value(&patch);
    assert_eq!(wire::patch_from_value(&value).expect("patch"), patch);

    let short = json!({
        "version": 1,
        "edits": [{
            "op": "set_tile", "layer": 0, "row_start": 0, "rows": 1,
            "col_start": 0, "cols": 2, "values": "+++"
        }]
    });
    assert!(wire::patch_from_value(&short).is_err());
}

#[test]
fn tile_edit_is_audited_and_replayable() {
    let original = sample();
    let mut net = original.clone();
    let patch = Patch::new(vec![Edit::FillTile {
        layer: 0,
        row_start: 1,
        rows: 2,
        col_start: 1,
        cols: 2,
        to: Trit::Pos,
    }]);
    let journal = patch.apply(&mut net).expect("apply");
    let mut log = AuditLog::new();
    log.record("tile-edit", &original, &net, patch.clone(), true, false)
        .expect("record");
    let (report, replayed) = log.replay_net(&original).expect("replay");
    assert_eq!(report.applied, 1);
    assert_eq!(replayed, net);
    assert_eq!(log.len(), 1);

    journal.undo(&mut net);
    assert_eq!(net, original);

    let mut rolled = original.clone();
    let journal = patch.apply(&mut rolled).expect("apply");
    journal.undo(&mut rolled);
    log.record("tile-rollback", &original, &rolled, patch, false, true)
        .expect("record rollback");
    assert_eq!(rolled, original);
    assert_eq!(log.len(), 2);
}

#[test]
fn cost_counts_tiles_and_touched_trits() {
    let net = sample();
    let patch = Patch::new(vec![
        Edit::FillTile {
            layer: 0,
            row_start: 0,
            rows: 2,
            col_start: 0,
            cols: 2,
            to: Trit::Pos,
        },
        Edit::SetTile {
            layer: 1,
            row_start: 0,
            rows: 1,
            col_start: 0,
            cols: 3,
            values: vec![Trit::Pos, Trit::Neg, Trit::Zero],
        },
    ]);
    let cost = patch.cost(&net);
    assert_eq!(cost.edits, 2);
    assert_eq!(cost.tile_ops, 2);
    assert_eq!(cost.touched, 4 + 3);
}
