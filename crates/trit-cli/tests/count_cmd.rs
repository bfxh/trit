//! `trit count` 的行为断言（跑真二进制，不是只测库函数）。
//!
//! 库层已经有三路对拍的测试；这里钉的是**命令行这一层**：目标值、二值域、
//! 以及"BDD 被预算拦下"必须报成 refused 而不能报成不一致。

use std::process::Command;

fn run(args: &[&str]) -> (String, String, i32) {
    let bin = std::env::var("CARGO_BIN_EXE_trit")
        .unwrap_or_else(|_| format!("trit{}", std::env::consts::EXE_SUFFIX));
    let out = Command::new(bin)
        .args(args)
        .output()
        .expect("failed to run trit binary");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

fn json(args: &[&str]) -> serde_json::Value {
    let (stdout, stderr, code) = run(args);
    assert_eq!(code, 0, "stderr={stderr}\nstdout={stdout}");
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{e}: {stdout}"))
}

/// Kleene 下 `a & b` 为真只有 1 个赋值；`a | b` 为真有 5 个；`a & b` 为**未定**有 3 个。
/// 三个数都由穷举独立复算过（`--crosscheck`），不是把 DPLL 的输出抄一遍。
#[test]
fn count_agrees_with_exhaustive_across_targets() {
    for (expr, target, want) in [
        ("a & b", "true", "1"),
        ("a | b", "true", "5"),
        ("a & b", "unknown", "3"),
    ] {
        let got = json(&[
            "count",
            "models",
            expr,
            "--target",
            target,
            "--crosscheck",
            "--json",
        ]);
        assert_eq!(got["count"], want, "{expr} = {target}");
        assert_eq!(got["crosscheck"]["exhaustive_count"], want, "{expr}");
        assert_eq!(got["crosscheck"]["matches"], true, "{expr}");
        assert_eq!(got["crosscheck"]["three_way"], true, "{expr}：BDD 也该出数");
    }
}

/// `--binary a` 把 a 的域从 {F,U,T} 收到 {F,T} ⇒ 计数必须变（5 → 4），
/// 且穷举按同一份二值域复算（否则这个开关只是被解析掉了）。
#[test]
fn binary_vars_narrow_the_domain() {
    let plain = json(&["count", "models", "a | b", "--crosscheck", "--json"]);
    let fixed = json(&[
        "count",
        "models",
        "a | b",
        "--binary",
        "a",
        "--crosscheck",
        "--json",
    ]);
    assert_eq!(plain["count"], "5");
    assert_eq!(fixed["count"], "4");
    assert_eq!(fixed["binary_vars"][0], "a");
    assert_eq!(fixed["crosscheck"]["matches"], true);
}

/// 前置约束收窄计数域：`(a | b)` 在 `¬a` 之下只剩 `b = T` 那一支
/// （Kleene 里 `F | U = U`，不等于 `T` ⇒ 未定那支不计数）。
#[test]
fn constraints_restrict_the_counted_assignments() {
    let got = json(&[
        "count",
        "models",
        "a | b",
        "--constraints",
        "!a",
        "--crosscheck",
        "--json",
    ]);
    assert_eq!(got["count"], "1", "a=F 且 a|b=T ⇒ 只有 b=T");
    assert_eq!(got["crosscheck"]["exhaustive_count"], got["count"]);
    assert_eq!(got["crosscheck"]["matches"], true);
}

/// BDD 预算拦下时：`matches` 仍可成立，但 `three_way` 必须为假，且拒绝原因要印出来。
/// 这一条挡住"把拒绝当不一致"（会让 CI 在合法的大公式上乱红）。
#[test]
fn bdd_budget_refusal_is_reported_as_refused_not_mismatch() {
    let got = json(&[
        "count",
        "models",
        "a & b",
        "--bdd-max-nodes",
        "1",
        "--crosscheck",
        "--json",
    ]);
    let cc = &got["crosscheck"];
    assert_eq!(cc["matches"], true);
    assert_eq!(cc["three_way"], false);
    assert!(cc["bdd_count"].is_null(), "被拦下时不该有 BDD 计数：{cc}");
    let reason = cc["bdd_refused"].as_str().unwrap_or_default();
    assert!(reason.contains("上限"), "拒绝原因要点名预算：{reason}");

    // 文本模式：拒绝要读得成一句话，不能印成 "BDD BDD 节点数超过上限"。
    let (stdout, stderr, code) = run(&[
        "count",
        "models",
        "a | b",
        "--bdd-max-nodes",
        "1",
        "--crosscheck",
    ]);
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(
        stdout.contains("BDD 拒（BDD 节点数超过上限 1）"),
        "{stdout}"
    );
    assert!(!stdout.contains("BDD BDD "), "前缀叠了两层：{stdout}");
}

/// 用法错误要非零退出并点名见到的值；空计数域要拒绝而不是静默返回 1。
#[test]
fn usage_errors_are_loud() {
    let (stdout, stderr, code) = run(&["count", "models", "a", "--target", "zebra"]);
    assert_ne!(code, 0, "stdout={stdout}");
    assert!(stderr.contains("zebra"), "报错要点名非法值：{stderr}");

    let (stdout, stderr, code) = run(&["count", "models", "1", "--json"]);
    assert_ne!(code, 0, "常量表达式应要求显式 --vars：{stdout}");
    assert!(stderr.contains("--vars"), "{stderr}");
}
