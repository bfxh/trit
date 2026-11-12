//! 梯度知情提议：STE 反向传播 top-K 与 `analyze` 实测对拍，以及 CLI。

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::Instant;

use serde_json::Value;
use trit_wspace::analyze::analyze;
use trit_wspace::learned;
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
fn gradient_top_k_matches_analyze() {
    let (broken, oracle) = problem(7);
    let start = Instant::now();
    let analysis = analyze(&broken, &oracle, 0).expect("analyze");
    let analyze_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    let scored = learned::gradient_scores(&broken, &oracle).expect("gradient");
    let gradient_ms = start.elapsed().as_secs_f64() * 1000.0;
    println!("analyze {analyze_ms:.3} ms, gradient {gradient_ms:.3} ms");
    assert_eq!(scored.len(), analysis.candidates.len());
    assert!(scored.windows(2).all(|pair| pair[0].score >= pair[1].score));

    let analyze_top: Vec<&Edit> = analysis
        .candidates
        .iter()
        .take(10)
        .map(|impact| &impact.edit)
        .collect();
    let mut gradient_hits = 0;
    let mut overlap = 0;
    for scored in scored.iter().take(10) {
        if let Some(impact) = analysis
            .candidates
            .iter()
            .find(|impact| impact.edit == scored.edit)
            && impact.gain > 0
        {
            gradient_hits += 1;
        }
        if analyze_top.contains(&&scored.edit) {
            overlap += 1;
        }
    }
    let analyze_hits = analysis
        .candidates
        .iter()
        .take(10)
        .filter(|impact| impact.gain > 0)
        .count();
    println!(
        "gradient top10 hits: {gradient_hits}, analyze top10 hits: {analyze_hits}, overlap: {overlap}"
    );
    assert!(gradient_hits > 0);
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("trit-gradient-{tag}-{}", std::process::id()));
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
fn gradient_cli_verifies_top_k() {
    let dir = temp_dir("cli");
    let (broken, oracle) = problem(11);
    let artifact = dir.join("model.wsp");
    let oracle_path = dir.join("oracle.json");
    fs::write(&artifact, to_wsp(&broken)).expect("wsp1");
    fs::write(
        &oracle_path,
        serde_json::to_string_pretty(&wire::oracle_to_value(&oracle)).expect("oracle json"),
    )
    .expect("write oracle");

    let report = stdout_json(&run(&[
        "propose-gradient",
        artifact.to_str().expect("artifact"),
        "--oracle",
        oracle_path.to_str().expect("oracle"),
        "--limit",
        "5",
        "--verify",
    ]));
    assert_eq!(report["method"], "ste-gradient");
    assert_eq!(
        report["candidates"].as_array().expect("candidates").len(),
        5
    );
    assert!(report["candidates"][0]["gain"].is_number());
}
