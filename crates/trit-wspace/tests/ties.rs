//! TIES trim：逐坐标按 |Δ| 裁剪、margin 门槛、default 等价与 CLI 参数。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use trit_wspace::task_merge::{
    TrimOptions, TrimTop, merge_tasks_weighted_report, merge_tasks_weighted_trim_report,
};
use trit_wspace::{Edit, Layer, Net, Patch, TaskVector, Trit, to_wsp};

fn single() -> Net {
    Net::new(vec![Layer::new(1, 1, &[0], &[0]).expect("layer")]).expect("net")
}

fn with(value: Trit) -> Net {
    let mut net = single();
    Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: value,
    }])
    .apply(&mut net)
    .expect("apply");
    net
}

fn task(base: &Net, value: Trit) -> TaskVector {
    TaskVector::between(base, &with(value)).expect("task vector")
}

fn trit_of(net: &Net) -> Trit {
    net.layer(0).expect("layer").trit(0, 0).expect("trit")
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("trit-wspace-ties-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_trit-wspace"))
        .args(args)
        .output()
        .expect("run cli")
}

fn stdout_json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("json stdout")
}

fn path_string(path: &Path) -> String {
    path.to_str().expect("utf-8 path").to_string()
}

#[test]
fn keep_counts_and_fractions() {
    assert_eq!(TrimOptions::default().keep(3), 3);
    assert_eq!(
        TrimOptions {
            top: Some(TrimTop::Count(2)),
            margin: 0.0
        }
        .keep(3),
        2
    );
    assert_eq!(
        TrimOptions {
            top: Some(TrimTop::Count(9)),
            margin: 0.0
        }
        .keep(3),
        3,
        "票数上限超过实际票数时等于不裁剪"
    );
    assert_eq!(
        TrimOptions {
            top: Some(TrimTop::Fraction(0.1)),
            margin: 0.0
        }
        .keep(3),
        1,
        "比例向上取整但至少保留一票"
    );
    assert_eq!(
        TrimOptions {
            top: Some(TrimTop::Fraction(0.6)),
            margin: 0.0
        }
        .keep(3),
        2
    );
    assert_eq!(
        TrimOptions {
            top: Some(TrimTop::Fraction(1.0)),
            margin: 0.0
        }
        .keep(3),
        3
    );
    assert!(TrimOptions::default().is_identity());
    assert!(
        !TrimOptions {
            top: None,
            margin: 0.5
        }
        .is_identity()
    );
}

#[test]
fn default_trim_matches_plain_path_bit_for_bit() {
    let base = single();
    let tasks = vec![
        task(&base, Trit::Pos),
        task(&base, Trit::Neg),
        task(&base, Trit::Pos),
    ];
    let weights = vec![1.0, 2.0, 0.5];
    let (plain, plain_report) =
        merge_tasks_weighted_report(&base, &tasks, &weights).expect("plain");
    let (trimmed, report) =
        merge_tasks_weighted_trim_report(&base, &tasks, &weights, &TrimOptions::default())
            .expect("trim");
    assert_eq!(
        plain.layers()[0].trits(),
        trimmed.layers()[0].trits(),
        "默认 TrimOptions 必须与旧路径逐位一致"
    );
    assert_eq!(plain.state_hash(), trimmed.state_hash());
    assert_eq!(plain_report.changed(), report.changed());
    assert_eq!(plain_report.ties(), report.ties());
    assert_eq!(plain_report.dropped(), report.dropped());
}

#[test]
fn count_trim_breaks_ties() {
    let base = single();
    // 三票 + / - / 0：票和为 0 且票值不全相同 ⇒ 无 trim 时是 tie，保留 base。
    let tasks = vec![
        task(&base, Trit::Pos),
        task(&base, Trit::Neg),
        task(&base, Trit::Zero),
    ];
    let weights = vec![1.0, 1.0, 1.0];

    let (plain, plain_report) =
        merge_tasks_weighted_report(&base, &tasks, &weights).expect("plain");
    assert_eq!(plain_report.ties(), 1);
    assert_eq!(plain_report.changed(), 0);
    assert_eq!(trit_of(&plain), Trit::Zero);

    let trim = TrimOptions {
        top: Some(TrimTop::Count(1)),
        margin: 0.0,
    };
    let (trimmed, report) =
        merge_tasks_weighted_trim_report(&base, &tasks, &weights, &trim).expect("trim");
    assert_eq!(report.ties(), 0, "裁剪后只剩一票，不可能平票");
    assert_eq!(report.changed(), 1);
    assert_eq!(report.dropped(), 0);
    assert_eq!(trit_of(&trimmed), Trit::Pos, "|Δ| 相同时按任务顺序取第一票");

    let half = TrimOptions {
        top: Some(TrimTop::Fraction(0.4)),
        margin: 0.0,
    };
    let (fraction, report) =
        merge_tasks_weighted_trim_report(&base, &tasks, &weights, &half).expect("fraction");
    assert_eq!(report.changed(), 1, "ceil(0.4 × 3) = 2 票，仍可定符号");
    assert_eq!(trit_of(&fraction), Trit::Pos);
}

#[test]
fn margin_drops_low_agreement() {
    let base = single();
    // 两票同向、一票反向：票和 ±1，低于 margin 1.5 时整条丢弃。
    let tasks = vec![
        task(&base, Trit::Pos),
        task(&base, Trit::Pos),
        task(&base, Trit::Neg),
    ];
    let weights = vec![1.0, 1.0, 1.0];
    let (merged, report) =
        merge_tasks_weighted_trim_report(&base, &tasks, &weights, &TrimOptions::default())
            .expect("plain");
    assert_eq!(trit_of(&merged), Trit::Pos);
    assert_eq!(report.changed(), 1);
    assert_eq!(report.dropped(), 0);

    let margin = TrimOptions {
        top: None,
        margin: 1.5,
    };
    let (kept, report) =
        merge_tasks_weighted_trim_report(&base, &tasks, &weights, &margin).expect("margin");
    assert_eq!(trit_of(&kept), Trit::Zero, "低于门槛时保留 base");
    assert_eq!(report.changed(), 0);
    assert_eq!(report.dropped(), 1);
}

#[test]
fn margin_counts_every_dropped_coordinate() {
    let base = Net::new(vec![Layer::new(2, 1, &[0, 0], &[0, 0]).expect("layer")]).expect("net");
    let mut tuned = base.clone();
    Patch::new(vec![
        Edit::SetTrit {
            layer: 0,
            row: 0,
            col: 0,
            to: Trit::Pos,
        },
        Edit::SetTrit {
            layer: 0,
            row: 1,
            col: 0,
            to: Trit::Neg,
        },
    ])
    .apply(&mut tuned)
    .expect("apply");
    let tasks = vec![TaskVector::between(&base, &tuned).expect("task")];
    let weights = vec![1.0];
    let margin = TrimOptions {
        top: None,
        margin: 2.0,
    };
    let (_, report) =
        merge_tasks_weighted_trim_report(&base, &tasks, &weights, &margin).expect("margin");
    assert_eq!(report.voted(), 2);
    assert_eq!(report.dropped(), 2, "两个坐标的票和都是 1，全部低于门槛");
    assert_eq!(report.changed(), 0);
}

#[test]
fn cli_trim_flags_and_dare_conflict() {
    let dir = temp_dir("cli");
    let base_path = dir.join("base.wsp");
    let base = single();
    fs::write(&base_path, to_wsp(&base)).expect("base");
    let mut tuned = Vec::new();
    for (name, value) in [("a", Trit::Pos), ("b", Trit::Pos), ("c", Trit::Neg)] {
        let path = dir.join(format!("{name}.wsp"));
        fs::write(&path, to_wsp(&with(value))).expect("tuned");
        tuned.push(path);
    }
    let mut args = vec![
        "task-merge".to_string(),
        path_string(&base_path),
        path_string(&tuned[0]),
        path_string(&tuned[1]),
        path_string(&tuned[2]),
    ];
    args.extend(["--trim".to_string(), "1".to_string()]);
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    let report = stdout_json(&run(&borrowed));
    assert_eq!(report["merge_report"]["changed"].as_u64(), Some(1));
    assert_eq!(report["merge_report"]["dropped"].as_u64(), Some(0));
    assert_eq!(report["trim"]["top"]["kind"], "count");
    assert_eq!(report["trim"]["top"]["value"].as_u64(), Some(1));
    assert_eq!(report["trim"]["margin"].as_f64(), Some(0.0));

    let conflict = run(&[
        borrowed[0],
        borrowed[1],
        borrowed[2],
        borrowed[3],
        borrowed[4],
        "--trim",
        "1",
        "--dare-rate",
        "0.5",
    ]);
    assert!(!conflict.status.success(), "trim 与 DARE 必须互斥");

    let fractional = run(&[
        borrowed[0],
        borrowed[1],
        borrowed[2],
        borrowed[3],
        borrowed[4],
        "--trim",
        "0.5",
    ]);
    let json = stdout_json(&fractional);
    assert_eq!(json["trim"]["top"]["kind"], "fraction");

    let bad = run(&[
        borrowed[0],
        borrowed[1],
        borrowed[2],
        borrowed[3],
        borrowed[4],
        "--trim",
        "1.5",
    ]);
    assert!(!bad.status.success(), "1.5 票不是合法值");

    let zero = run(&[
        borrowed[0],
        borrowed[1],
        borrowed[2],
        borrowed[3],
        borrowed[4],
        "--trim",
        "0",
    ]);
    assert!(!zero.status.success(), "0 票不是合法值");
}
