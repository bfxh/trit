//! `repair` 上界：先用 `localize` 把搜索缩到承重行与 top 单点，再搜单点/双点补丁。
//!
//! 384×384 的全量 `repair` 束搜索跑不动（147,456 个 trit × 8 条 edit 样本）；
//! 这里用两级定位（行探针 → 行内 trit 扫描）把候选压到几十个，再逐个/两两
//! 用编辑集与保持集一起验证，给出「最少改几个 trit 能达成 edit 集」的上界。
//!
//! 任务定义与 `editset_gate` 一致：真实 `word_embeddings` 行做输入、f32 `query`
//! 层做教师、三值 `query` + 三值读出层做学生。
//!
//! ```bash
//! cargo run --release -p trit-wspace --example editset_bound -- \
//!   --safetensors model.safetensors --samples 512 \
//!   --edit-limit 8 --keep-limit 64 --top-rows 8 --top-trits 32
//! ```

use std::collections::BTreeMap;

use serde_json::json;
use trit_wspace::safetensors::{self, Weights};
use trit_wspace::{Edit, Layer, Net, Oracle, Patch, Trit, localize};

const QUERY: &str = "encoder.layer.0.attention.self.query.weight";
const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";

struct Args {
    safetensors: String,
    query: String,
    embeddings: String,
    readout_row: usize,
    samples: usize,
    edit_limit: usize,
    keep_limit: usize,
    seed: u64,
    row_budget: usize,
    top_rows: usize,
    max_trits: usize,
    top_trits: usize,
    pair_limit: usize,
    out: Option<String>,
}

fn parse_args() -> Args {
    let mut args = Args {
        safetensors: String::new(),
        query: QUERY.to_string(),
        embeddings: EMBEDDINGS.to_string(),
        readout_row: 0,
        samples: 512,
        edit_limit: 8,
        keep_limit: 64,
        seed: 7,
        row_budget: 64,
        top_rows: 8,
        max_trits: 64,
        top_trits: 32,
        pair_limit: 24,
        out: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut next = || it.next().expect("缺少参数值");
        match arg.as_str() {
            "--safetensors" => args.safetensors = next(),
            "--query" => args.query = next(),
            "--embeddings" => args.embeddings = next(),
            "--readout-row" => args.readout_row = next().parse().expect("readout-row"),
            "--samples" => args.samples = next().parse().expect("samples"),
            "--edit-limit" => args.edit_limit = next().parse().expect("edit-limit"),
            "--keep-limit" => args.keep_limit = next().parse().expect("keep-limit"),
            "--seed" => args.seed = next().parse().expect("seed"),
            "--row-budget" => args.row_budget = next().parse().expect("row-budget"),
            "--top-rows" => args.top_rows = next().parse().expect("top-rows"),
            "--max-trits" => args.max_trits = next().parse().expect("max-trits"),
            "--top-trits" => args.top_trits = next().parse().expect("top-trits"),
            "--pair-limit" => args.pair_limit = next().parse().expect("pair-limit"),
            "--out" => args.out = Some(next()),
            other => panic!("未知参数 {other}"),
        }
    }
    assert!(!args.safetensors.is_empty(), "需要 --safetensors");
    args
}

fn sign_of(score: f32) -> Trit {
    if score > 0.0 {
        Trit::Pos
    } else if score < 0.0 {
        Trit::Neg
    } else {
        Trit::Zero
    }
}

fn trit_row(values: &[f32]) -> Vec<Trit> {
    let weights = Weights {
        rows: 1,
        cols: values.len(),
        values: values.to_vec(),
    };
    let layer = weights.to_layer().expect("ternarize row");
    (0..values.len())
        .map(|col| layer.trit(0, col).expect("trit"))
        .collect()
}

fn readout_layer(readout: &[f32]) -> Layer {
    let trits: Vec<i8> = trit_row(readout).iter().map(|t| t.to_i8()).collect();
    Layer::new(1, readout.len(), &trits, &[0]).expect("readout layer")
}

fn teacher_forward(query: &Weights, x: &[f32]) -> Vec<f32> {
    let mut out = vec![0.0f32; query.rows];
    for (row, slot) in out.iter_mut().enumerate() {
        let weights = &query.values[row * query.cols..(row + 1) * query.cols];
        *slot = weights
            .iter()
            .zip(x)
            .map(|(weight, value)| weight * value)
            .sum();
    }
    out
}

fn dot(readout: &[f32], values: &[f32]) -> f32 {
    readout.iter().zip(values).map(|(r, v)| r * v).sum()
}

fn edit_key(edit: &Edit) -> (usize, u8, usize, usize) {
    match *edit {
        Edit::SetTrit {
            layer, row, col, ..
        } => (layer, 0, row, col),
        Edit::SetBias { layer, row, .. } => (layer, 1, row, 0),
        _ => (usize::MAX, 2, 0, 0),
    }
}

fn score(net: &Net, edit_set: &Oracle, keep_set: &Oracle, edits: &[Edit]) -> (usize, usize) {
    let mut work = net.clone();
    Patch::new(edits.to_vec()).apply(&mut work).expect("apply");
    (
        edit_set.loss(&work).expect("edit loss"),
        keep_set.loss(&work).expect("keep loss"),
    )
}

fn main() {
    let args = parse_args();
    let query = safetensors::read_tensor(&args.safetensors, &args.query).expect("query tensor");
    let embeddings =
        safetensors::read_tensor(&args.safetensors, &args.embeddings).expect("embeddings tensor");
    assert_eq!(query.cols, embeddings.cols, "query 列数必须等于词向量维度");
    assert!(args.readout_row < query.rows, "--readout-row 越界");
    let dim = query.cols;
    let readout = &query.values[args.readout_row * dim..(args.readout_row + 1) * dim];
    let student = Net::new(vec![
        query.to_layer().expect("ternary query"),
        readout_layer(readout),
    ])
    .expect("student net");

    let mut state = args.seed ^ 0x9E37_79B9_7F4A_7C15;
    if state == 0 {
        state = 1;
    }
    let mut next = || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };

    let budget = args.samples.max(args.edit_limit + args.keep_limit);
    let mut labelled = 0usize;
    let mut edit_inputs = Vec::new();
    let mut edit_outputs = Vec::new();
    let mut keep_inputs = Vec::new();
    let mut keep_outputs = Vec::new();
    while labelled < budget {
        let row = (next() % embeddings.rows as u64) as usize;
        let x_f32 = &embeddings.values[row * dim..(row + 1) * dim];
        let teacher = sign_of(dot(readout, &teacher_forward(&query, x_f32)));
        if teacher == Trit::Zero {
            continue;
        }
        labelled += 1;
        let x_trit = trit_row(x_f32);
        let student_label = student.forward(&x_trit).expect("forward")[0];
        if student_label == teacher {
            if keep_inputs.len() < args.keep_limit {
                keep_inputs.push(x_trit);
                keep_outputs.push(vec![teacher]);
            }
        } else if edit_inputs.len() < args.edit_limit {
            edit_inputs.push(x_trit);
            edit_outputs.push(vec![teacher]);
        }
        if edit_inputs.len() >= args.edit_limit && keep_inputs.len() >= args.keep_limit {
            break;
        }
    }
    assert!(
        !edit_inputs.is_empty() && !keep_inputs.is_empty(),
        "edit/keep 集为空，无法给上界"
    );
    let edit = Oracle::new(edit_inputs, edit_outputs).expect("edit oracle");
    let keep = Oracle::new(keep_inputs, keep_outputs).expect("keep oracle");
    let baseline = (
        edit.loss(&student).expect("loss"),
        keep.loss(&student).expect("loss"),
    );

    let located = localize::localize(
        &student,
        &edit,
        args.row_budget,
        args.top_rows,
        args.max_trits,
        args.top_trits,
    )
    .expect("localize");

    // 同一坐标只保留 gain 最大的目标值。
    let mut best_by_target: BTreeMap<(usize, u8, usize, usize), &trit_wspace::TritImpact> =
        BTreeMap::new();
    for row in &located.rows {
        for impact in &row.top {
            let key = edit_key(&impact.edit);
            best_by_target
                .entry(key)
                .and_modify(|current| {
                    if impact.gain > current.gain {
                        *current = impact;
                    }
                })
                .or_insert(impact);
        }
    }
    let candidates: Vec<&Edit> = best_by_target.values().map(|impact| &impact.edit).collect();

    let mut single = Vec::new();
    for candidate in &candidates {
        let (edit_loss, keep_loss) = score(&student, &edit, &keep, &[(*candidate).clone()]);
        single.push((edit_loss, keep_loss, *candidate));
    }
    single.sort_by_key(|(edit_loss, keep_loss, _)| (*edit_loss, *keep_loss));
    let best_single = single.first().cloned();
    let single_reachable = best_single.is_some_and(|(edit_loss, _, _)| edit_loss == 0);

    let mut best_pair = None;
    let mut pairs = 0usize;
    if !single_reachable {
        let head: Vec<&Edit> = candidates.iter().take(args.pair_limit).copied().collect();
        for left in 0..head.len() {
            for right in (left + 1)..head.len() {
                pairs += 1;
                let (edit_loss, keep_loss) = score(
                    &student,
                    &edit,
                    &keep,
                    &[head[left].clone(), head[right].clone()],
                );
                let better = edit_loss < best_single.map_or(usize::MAX, |(loss, _, _)| loss)
                    && best_pair.as_ref().is_none_or(
                        |(best_edit, best_keep, _, _): &(usize, usize, Edit, Edit)| {
                            (edit_loss, keep_loss) < (*best_edit, *best_keep)
                        },
                    );
                if better {
                    best_pair = Some((
                        edit_loss,
                        keep_loss,
                        head[left].clone(),
                        head[right].clone(),
                    ));
                }
            }
        }
    }
    let pair_reachable = best_pair
        .as_ref()
        .is_some_and(|(edit_loss, _, _, _)| *edit_loss == 0);

    let mut report = json!({
        "safetensors": args.safetensors,
        "readout_row": args.readout_row,
        "samples": labelled,
        "edit_set": edit.len(),
        "keep_set": keep.len(),
        "baseline": {"edit_loss": baseline.0, "keep_loss": baseline.1},
        "localize": {
            "rows": located.rows.len(),
            "candidates": candidates.len(),
            "probed": located.rows.iter().map(|row| row.probed).sum::<usize>(),
        },
        "best_single": best_single.map(|(edit_loss, keep_loss, edit)| json!({
            "edit_loss": edit_loss,
            "keep_loss": keep_loss,
            "edit": format!("{edit:?}"),
        })),
        "best_pair": best_pair.map(|(edit_loss, keep_loss, left, right)| json!({
            "edit_loss": edit_loss,
            "keep_loss": keep_loss,
            "edits": [format!("{left:?}"), format!("{right:?}")],
        })),
        "pairs_evaluated": pairs,
        "reachable": single_reachable || pair_reachable,
        "reachable_with": if single_reachable { json!("single") } else if pair_reachable { json!("pair") } else { json!(null) },
    });
    report["keep_break_rate"] =
        json!(best_single.map(|(_, keep_loss, _)| keep_loss as f64 / keep.len() as f64));
    let text = serde_json::to_string_pretty(&report).expect("json");
    if let Some(path) = &args.out {
        std::fs::write(path, &text).expect("write report");
    }
    println!("{text}");
}
