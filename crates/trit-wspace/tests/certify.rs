//! 保持集副作用证书：违规检出、五分类、空操作/重复目标、编辑集门禁、CLI 端到端。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use trit_wspace::certify::{self, CertifyOptions, Verdict};
use trit_wspace::{Edit, Layer, Net, Oracle, Patch, Trit, to_wsp};

fn xor_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("xor net")
}

fn xor_oracle() -> Oracle {
    Oracle::boolean(2, |b| b[0] != b[1]).expect("xor oracle")
}

fn break_patch() -> Patch {
    Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Zero,
    }])
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("trit-wspace-certify-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_trit-wspace")
}

fn run(args: &[&str]) -> Output {
    Command::new(bin()).args(args).output().expect("run cli")
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
fn harmless_scale_patch_certifies_clean() {
    let net = xor_net();
    let keep = xor_oracle();
    let patch = Patch::new(vec![Edit::SetScale {
        layer: 0,
        row: 0,
        to: 2.0,
    }]);
    let certificate =
        certify::certify(&net, &patch, &keep, Some(&keep), &CertifyOptions::default())
            .expect("certify");
    assert!(certificate.passed());
    assert_eq!(certificate.keep.samples, 4);
    assert_eq!(certificate.keep.preserved, 4);
    assert_eq!(certificate.keep.violations(), 0);
    assert_eq!(
        certificate.edit.as_ref().expect("edit").after_loss,
        0,
        "编辑集必须同时达成"
    );
    let facts = certificate.patch.expect("facts");
    assert_eq!(facts.edits, 1);
    assert_eq!(facts.scales, 1);
    assert_eq!(facts.noop, 0);
    assert_eq!(facts.touched, 0, "scale 不计入 touched");
}

#[test]
fn breaking_patch_is_flagged_as_regressed() {
    let net = xor_net();
    let keep = xor_oracle();
    let certificate = certify::certify(
        &net,
        &break_patch(),
        &keep,
        None,
        &CertifyOptions::default(),
    )
    .expect("certify");
    assert!(!certificate.passed());
    assert!(certificate.keep.violations() > 0);
    assert_eq!(certificate.keep.after_loss, certificate.keep.violations());
    assert_eq!(
        certificate.keep.preserved + certificate.keep.regressed,
        certificate.keep.samples
    );
    let row = certificate.keep.rows.first().expect("violation row");
    assert_eq!(row.verdict, Verdict::Regressed);
    assert_eq!(row.verdict.label(), "regressed");
    assert_eq!(row.want, row.before, "违规样本编辑前是对的");
    assert_ne!(row.want, row.after, "违规样本编辑后错了");
    let json = certificate.to_json();
    assert_eq!(json["schema"], "trit-wspace.certificate.v1");
    assert_eq!(json["certified"], false);
    assert_eq!(json["keep"]["rows"][0]["verdict"], "regressed");
}

#[test]
fn noop_and_repeated_targets_are_counted() {
    let net = xor_net();
    let current = net
        .layer(0)
        .expect("layer")
        .trit(0, 0)
        .expect("trit in range");
    let patch = Patch::new(vec![
        Edit::SetTrit {
            layer: 0,
            row: 0,
            col: 0,
            to: current,
        },
        Edit::SetTrit {
            layer: 0,
            row: 0,
            col: 0,
            to: Trit::Zero,
        },
        Edit::SetTrit {
            layer: 0,
            row: 0,
            col: 0,
            to: current,
        },
    ]);
    let facts = certify::patch_facts(&net, &patch);
    assert_eq!(facts.edits, 3);
    assert_eq!(facts.noop, 2, "首尾两条目标值已等于当前值");
    assert_eq!(facts.repeated, 2, "同一位置被写第二、三次");
    assert_eq!(facts.trits, 3);
}

#[test]
fn unmet_edit_set_blocks_certificate_even_with_allowance() {
    let net = xor_net();
    let oracle = xor_oracle();
    let options = CertifyOptions { allow: 99, rows: 8 };
    // 保持集用同一份 oracle：violations 会被 allow 放行，但编辑集没达成。
    let certificate =
        certify::certify(&net, &break_patch(), &oracle, Some(&oracle), &options).expect("certify");
    assert!(certificate.keep.violations() <= certificate.allow);
    assert!(
        certificate.edit.as_ref().expect("edit").after_loss > 0,
        "破坏补丁不可能达成编辑集"
    );
    assert!(!certificate.passed(), "编辑集未达成时证书不成立");
}

#[test]
fn verdicts_cover_fixed_drifted_stale() {
    let base = Net::new(vec![Layer::new(1, 1, &[1], &[0]).expect("layer")]).expect("base");
    let edited = Net::new(vec![Layer::new(1, 1, &[-1], &[0]).expect("layer")]).expect("edited");
    let input = vec![Trit::Pos];
    let before = base.forward(&input).expect("before");
    let after = edited.forward(&input).expect("after");
    assert_ne!(before, after, "两份权重必须在这一点上行为不同");
    let options = CertifyOptions::default();

    let fixed = certify::certify_nets(
        &base,
        &edited,
        &Oracle::new(vec![input.clone()], vec![after.clone()]).expect("oracle"),
        None,
        &options,
    )
    .expect("nets");
    assert_eq!(fixed.keep.fixed, 1);
    assert!(fixed.passed());

    let regressed = certify::certify_nets(
        &base,
        &edited,
        &Oracle::new(vec![input.clone()], vec![before.clone()]).expect("oracle"),
        None,
        &options,
    )
    .expect("nets");
    assert_eq!(regressed.keep.regressed, 1);

    let drifted = certify::certify_nets(
        &base,
        &edited,
        &Oracle::new(vec![input.clone()], vec![vec![Trit::Zero]]).expect("oracle"),
        None,
        &options,
    )
    .expect("nets");
    assert_eq!(drifted.keep.drifted, 1);
    assert_eq!(drifted.keep.before_loss, 1);
    assert_eq!(drifted.keep.after_loss, 1);

    let stale = certify::certify_nets(
        &base,
        &base,
        &Oracle::new(vec![input], vec![vec![Trit::Zero]]).expect("oracle"),
        None,
        &options,
    )
    .expect("nets");
    assert_eq!(stale.keep.stale, 1);
    assert!(stale.patch.is_none(), "直接比两个工件时没有补丁事实");
}

#[test]
fn cli_certify_gates_and_accepts_edited_artifact() {
    let dir = temp_dir("cli");
    let artifact = dir.join("xor.wsp");
    let oracle = dir.join("xor.json");
    let patch = dir.join("break.json");
    let edited = dir.join("edited.wsp");
    let certificate = dir.join("certificate.json");
    fs::write(&artifact, to_wsp(&xor_net())).expect("artifact");
    fs::write(&oracle, r#"{"domain":"boolean","vars":2,"table":"0110"}"#).expect("oracle");
    fs::write(
        &patch,
        r#"{"version":1,"edits":[{"op":"set_trit","layer":0,"row":0,"col":0,"to":"0"}]}"#,
    )
    .expect("patch");

    let report = stdout_json(&run(&[
        "certify",
        &path_string(&artifact),
        "--patch",
        &path_string(&patch),
        "--keep",
        &path_string(&oracle),
        "--edit",
        &path_string(&oracle),
        "--out",
        &path_string(&edited),
        "--certificate",
        &path_string(&certificate),
    ]));
    assert_eq!(report["certified"], false);
    assert_eq!(report["schema"], "trit-wspace.certificate.v1");
    assert!(report["keep"]["regressed"].as_u64().expect("count") > 0);
    assert!(report["edit"]["after_loss"].as_u64().expect("count") > 0);
    assert!(certificate.exists(), "证书必须落盘");
    assert!(edited.exists(), "编辑后工件必须落盘");

    let strict = run(&[
        "certify",
        &path_string(&artifact),
        "--patch",
        &path_string(&patch),
        "--keep",
        &path_string(&oracle),
        "--strict",
    ]);
    assert!(!strict.status.success(), "--strict 遇到违规必须非零退出");

    let allowed = stdout_json(&run(&[
        "certify",
        &path_string(&artifact),
        "--edited",
        &path_string(&edited),
        "--keep",
        &path_string(&oracle),
        "--allow",
        "99",
    ]));
    assert_eq!(allowed["certified"], true);
    assert_eq!(allowed["patch"], Value::Null);

    let neither = run(&[
        "certify",
        &path_string(&artifact),
        "--keep",
        &path_string(&oracle),
    ]);
    assert!(!neither.status.success(), "--patch 与 --edited 必须二选一");
}

#[test]
fn behavior_set_flags_weight_change_and_passes_scale_change() {
    let net = xor_net();
    let keep = certify::behavior_set(&net, 64, 7).expect("behavior set");
    assert_eq!(keep.len(), 64);
    assert!(keep.exact(&net).expect("exact"), "期望输出就是网络当前行为");

    // 只改 f32 scale：行为一字不变，证书必须放行。
    let scale_patch = Patch::new(vec![Edit::SetScale {
        layer: 0,
        row: 0,
        to: 3.0,
    }]);
    let mut same = net.clone();
    scale_patch.apply(&mut same).expect("apply scale");
    let clean = certify::certify_nets(&net, &same, &keep, None, &CertifyOptions::default())
        .expect("certify scale");
    assert!(clean.passed());
    assert_eq!(clean.keep.preserved, 64);

    // 改一格三值：行为漂移必须被记成 violation。
    let mut broken = net.clone();
    break_patch().apply(&mut broken).expect("apply break");
    let caught = certify::certify_nets(&net, &broken, &keep, None, &CertifyOptions::default())
        .expect("certify break");
    assert!(caught.keep.violations() > 0, "行为漂移必须被捕获");
    assert!(!caught.passed());
}
