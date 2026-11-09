//! 真实 `trit-train` 产物接入：`TernaryLinear` 训练 → WSP1 → 读回 → 任务合并/调权。
//!
//! 运行：`cargo run -p trit-wspace --example trit_train_tasks -- --out <dir>`

use std::fs;
use std::path::PathBuf;

use serde_json::json;
use trit_train::layer::TernaryLinear;
use trit_train::quant::Ste;
use trit_wspace::{
    Layer, Net, Oracle, TaskVector, Trit, from_wsp, merge_tasks_weighted, to_wsp, tune_task_weights,
};

const INPUTS: [[f32; 2]; 4] = [[1.0, 1.0], [1.0, -1.0], [-1.0, 1.0], [-1.0, -1.0]];
const LABELS: [usize; 4] = [0, 0, 1, 1];

fn one_hot(label: usize) -> [f32; 2] {
    if label == 0 { [1.0, 0.0] } else { [0.0, 1.0] }
}

fn train(indices: &[usize], epochs: usize) -> TernaryLinear {
    let mut layer = TernaryLinear::new(
        2,
        2,
        |r, c| if r == c { 0.5 } else { -0.5 },
        Ste::Identity,
        0.05,
        1.0,
    );
    for _ in 0..epochs {
        for &index in indices {
            layer.train_step(&INPUTS[index], &one_hot(LABELS[index]));
        }
    }
    layer
}

fn to_net(layer: &TernaryLinear) -> Net {
    let rows = layer.rows();
    let cols = layer.cols();
    let flat: Vec<i8> = layer.quantized().into_iter().flatten().collect();
    Net::new(vec![
        Layer::new(rows, cols, &flat, &vec![0i8; rows]).expect("layer"),
    ])
    .expect("net")
}

fn trit(value: f32) -> Trit {
    if value > 0.0 { Trit::Pos } else { Trit::Neg }
}

fn main() {
    let mut out = std::env::temp_dir().join(format!("trit-train-artifacts-{}", std::process::id()));
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--out" {
            out = PathBuf::from(args.next().expect("--out 需要目录"));
        }
    }
    fs::create_dir_all(&out).expect("out dir");

    let base = to_net(&train(&[0, 1, 2, 3], 400));
    let a = to_net(&train(&[0, 1], 400));
    let b = to_net(&train(&[2, 3], 400));
    for (name, net) in [("base", &base), ("a", &a), ("b", &b)] {
        fs::write(out.join(format!("{name}.wsp")), to_wsp(net)).expect("write artifact");
    }

    // 读回真实 WSP1 产物，后续合并只碰文件产物。
    let base =
        from_wsp(&fs::read_to_string(out.join("base.wsp")).expect("read base")).expect("parse");
    let a = from_wsp(&fs::read_to_string(out.join("a.wsp")).expect("read a")).expect("parse");
    let b = from_wsp(&fs::read_to_string(out.join("b.wsp")).expect("read b")).expect("parse");
    let tasks = [
        TaskVector::between(&base, &a).expect("a vector"),
        TaskVector::between(&base, &b).expect("b vector"),
    ];
    let oracle = Oracle::new(
        INPUTS
            .iter()
            .map(|input| input.iter().map(|value| trit(*value)).collect())
            .collect(),
        LABELS
            .iter()
            .map(|label| one_hot(*label).iter().map(|value| trit(*value)).collect())
            .collect(),
    )
    .expect("oracle");
    let equal = merge_tasks_weighted(&base, &tasks, &[1.0, 1.0]).expect("equal");
    let tuned = tune_task_weights(&base, &tasks, &oracle, 2).expect("tune");
    let tuned_net = merge_tasks_weighted(&base, &tasks, tuned.weights()).expect("tuned merge");

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "artifacts": out.display().to_string(),
            "base": oracle.accuracy(&base).expect("base accuracy"),
            "equal": oracle.accuracy(&equal).expect("equal accuracy"),
            "tuned": oracle.accuracy(&tuned_net).expect("tuned accuracy"),
            "weights": tuned.weights(),
            "task_edits": [tasks[0].edits().len(), tasks[1].edits().len()],
        }))
        .expect("json")
    );
}
