use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{Value, json};
use trit_wspace::analyze::analyze;
use trit_wspace::learned::{self, FEATURE_NAMES, Scorer};
use trit_wspace::wire;
use trit_wspace::{Edit, Net, Oracle, Patch, Trit, to_wsp};

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn problem(seed: u64) -> (Net, Oracle) {
    let correct = Net::random(&[4, 8, 1], seed).expect("net");
    let oracle = Oracle::sampled_behavior(&correct, 4, 64, seed ^ 0x9e37_79b9).expect("oracle");
    let mut broken = correct.clone();
    let mut rng = Rng::new(seed ^ 0x5eed);
    let break_one = |broken: &mut Net, rng: &mut Rng| {
        let layer = (rng.next_u64() % 2) as usize;
        let target = &broken.layers()[layer];
        let row = (rng.next_u64() as usize) % target.rows();
        let col = (rng.next_u64() as usize) % target.cols();
        let current = target.trit(row, col).expect("trit");
        let to = if current == Trit::Pos {
            Trit::Neg
        } else {
            Trit::Pos
        };
        Patch::new(vec![Edit::SetTrit {
            layer,
            row,
            col,
            to,
        }])
        .apply(broken)
        .expect("apply");
    };
    for _ in 0..8 {
        break_one(&mut broken, &mut rng);
    }
    let mut changed = 0;
    while oracle.loss(&broken).expect("loss") == 0 {
        assert!(changed < 32, "无法通过少量单点编辑制造失配");
        break_one(&mut broken, &mut rng);
        changed += 1;
    }
    (broken, oracle)
}

#[test]
fn learns_on_a_broken_net() {
    let (broken, oracle) = problem(7);
    let analysis = analyze(&broken, &oracle, 0).expect("analyze");
    assert!(analysis.before_loss > 0, "打坏的网络应存在失配");
    let samples = learned::samples_from_analysis(&broken, &oracle, &analysis).expect("samples");
    assert_eq!(samples.len(), analysis.candidates.len());
    assert!(
        samples
            .iter()
            .all(|sample| sample.features.len() == FEATURE_NAMES.len())
    );
    assert!(
        samples
            .iter()
            .all(|sample| (-1.0..=1.0).contains(&sample.target))
    );

    let (scorer, report) = learned::train_holdout(&samples, 0.5, 300, 0.05, 7);
    println!("report: {report:?}");
    assert_eq!(report.train + report.holdout, samples.len());
    assert!(report.train > 0);
    assert!(report.holdout > 0);

    let scored = learned::propose(&broken, &oracle, &scorer, 10).expect("propose");
    assert_eq!(scored.len(), 10);
    assert!(scored.windows(2).all(|pair| pair[0].score >= pair[1].score));

    let value = scorer.to_json();
    assert_eq!(Scorer::from_json(&value).expect("from json"), scorer);

    let mut verified = 0;
    for scored in &scored {
        let mut trial = broken.clone();
        Patch::new(vec![scored.edit.clone()])
            .apply(&mut trial)
            .expect("apply");
        if oracle.loss(&trial).expect("loss") < analysis.before_loss {
            verified += 1;
        }
    }
    println!("top10 verified: {verified}");
    assert!(verified > 0);
}

#[test]
fn rejects_bad_models() {
    let good = json!({
        "magic": learned::MAGIC,
        "version": learned::VERSION,
        "features": FEATURE_NAMES.len(),
        "weights": vec![0.0f32; FEATURE_NAMES.len()],
        "bias": 0.0,
        "samples": 1,
        "epochs": 1,
        "lr": 0.05,
    });
    assert!(Scorer::from_json(&good).is_ok());

    let mut bad = good.clone();
    bad["magic"] = json!("X");
    assert!(Scorer::from_json(&bad).is_err());

    let mut bad = good.clone();
    bad["weights"] = json!([0.0]);
    assert!(Scorer::from_json(&bad).is_err());

    let mut bad = good;
    bad["bias"] = json!(f32::NAN);
    assert!(Scorer::from_json(&bad).is_err());
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("trit-learned-{tag}-{}", std::process::id()));
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
    serde_json::from_slice(&output.stdout).expect("stdout json")
}

#[test]
fn cli_train_then_learned_propose() {
    let dir = temp_dir("cli");
    let (broken, oracle) = problem(11);
    let artifact = dir.join("model.wsp");
    let oracle_path = dir.join("oracle.json");
    let model = dir.join("model.json");
    fs::write(&artifact, to_wsp(&broken)).expect("wsp1");
    fs::write(
        &oracle_path,
        serde_json::to_string_pretty(&wire::oracle_to_value(&oracle)).expect("oracle json"),
    )
    .expect("write oracle");

    let report = stdout_json(&run(&[
        "train-proposer",
        artifact.to_str().expect("artifact"),
        "--oracle",
        oracle_path.to_str().expect("oracle"),
        "--out",
        model.to_str().expect("model"),
        "--epochs",
        "200",
    ]));
    assert!(report["report"]["samples"].as_u64().expect("samples") > 0);
    assert!(model.exists());

    let report = stdout_json(&run(&[
        "propose-learned",
        artifact.to_str().expect("artifact"),
        "--oracle",
        oracle_path.to_str().expect("oracle"),
        "--model",
        model.to_str().expect("model"),
        "--limit",
        "5",
        "--verify",
    ]));
    assert_eq!(report["verified"], true);
    assert_eq!(
        report["candidates"].as_array().expect("candidates").len(),
        5
    );
    assert!(report["candidates"][0]["gain"].is_number());
}
