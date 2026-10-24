//! CLI 集成测试（续）：netlist / timing / atpg / 输出格式。

//! CLI 集成测试：调用编译好的二进制，验证各子命令的实际行为。

use std::process::Command;

/// 二进制路径（由 Cargo 在集成测试时注入）。
fn trit_bin() -> std::path::PathBuf {
    // target/debug/trit.exe；CARGO_BIN_EXE_<name> 环境变量更可靠
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_trit") {
        return std::path::PathBuf::from(p);
    }
    let mut dir = std::env::current_exe().expect("current exe");
    dir.pop(); // deps
    if dir.ends_with("deps") {
        dir.pop();
    }
    dir.join(format!("trit{}", std::env::consts::EXE_SUFFIX))
}

/// 运行 CLI，返回 (stdout, stderr, 退出码)。
fn run(args: &[&str]) -> (String, String, i32) {
    let out = Command::new(trit_bin())
        .args(args)
        .output()
        .expect("failed to run trit binary");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn netlist_converts_text_to_json() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/and2.net");
    let (stdout, stderr, code) = run(&["netlist", path.to_str().unwrap()]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("\"inputs\""), "{stdout}");
    assert!(stdout.contains("\"And\""), "{stdout}");
}

#[test]
fn bdd_max_nodes_is_enforced() {
    let (stdout, stderr, code) = run(&[
        "equiv",
        "(a & b) | (c & d)",
        "(a & b) | (d & c)",
        "--bdd",
        "--bdd-max-nodes",
        "1",
    ]);
    assert_ne!(code, 0, "stdout={stdout}");
    assert!(stderr.contains("BDD 节点数超过上限"), "stderr={stderr}");
}

#[test]
fn verify_best_order_matches_appearance() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/admission.json");
    let (stdout, stderr, code) = run(&[
        "verify",
        path.to_str().unwrap(),
        "--engine",
        "bdd",
        "--bdd-order",
        "best",
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("OK"), "{stdout}");
    assert!(stdout.contains("2 条断言"), "{stdout}");
}

#[test]
fn verify_best_rejects_sat_engine() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/admission.json");
    let (_, stderr, code) = run(&[
        "verify",
        path.to_str().unwrap(),
        "--engine",
        "sat",
        "--bdd-order",
        "best",
    ]);
    assert_ne!(code, 0);
    assert!(stderr.contains("只适用于 --engine bdd"), "{stderr}");
}

#[test]
fn netlist_converts_to_verilog() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/and2.net");
    let (stdout, stderr, code) = run(&["netlist", path.to_str().unwrap(), "--to", "verilog"]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("module netlist"), "{stdout}");
    assert!(stdout.contains("and g0"), "{stdout}");
}

#[test]
fn timing_reports_critical_path_and_slack() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chain3.net");
    let (stdout, stderr, code) = run(&["timing", path.to_str().unwrap(), "--clock", "1"]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("关键路径 2"), "{stdout}");
    assert!(stdout.contains("slack=-1"), "{stdout}");
    assert!(stdout.contains("违例 1"), "{stdout}");
}

#[test]
fn atpg_json_exposes_x_compaction() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/and2.net");
    let (stdout, stderr, code) = run(&["atpg", path.to_str().unwrap(), "--format", "json"]);
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(stdout.contains("\"x_coverage\""), "{stdout}");
    assert!(stdout.contains("\"merged\""), "{stdout}");
    assert!(stdout.contains("\"dont_care_bits\""), "{stdout}");
}

#[test]
fn atpg_compacts_x_patterns_and_exports_binary() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/buf3_wide.net");
    let path = path.to_str().unwrap();

    let (stdout, stderr, code) = run(&["atpg", path]);
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(stdout.contains("测试向量 2 个"), "{stdout}");
    assert!(stdout.contains("压缩省 2 个向量"), "{stdout}");
    assert!(stdout.contains("X 覆盖率 100.0%"), "{stdout}");

    let (stdout, stderr, code) = run(&["atpg", path, "--format", "csv"]);
    assert_eq!(code, 0, "stderr={stderr}");
    for line in stdout.lines().skip(1) {
        for cell in line.split(',').skip(1).take(9) {
            assert!(cell == "0" || cell == "1", "CSV 必须是具体 0/1：{cell}");
        }
    }
}

#[test]
fn atpg_accepts_verilog() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/and2.v");
    let (stdout, stderr, code) = run(&["atpg", path.to_str().unwrap(), "--from", "verilog"]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("覆盖率 100.0%"), "{stdout}");
}

#[test]
fn timing_uses_library_delays() {
    let fixtures = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let circuit = fixtures.join("chain3.net");
    let library = fixtures.join("timing.json");
    let (stdout, stderr, code) = run(&[
        "timing",
        circuit.to_str().unwrap(),
        "--library",
        library.to_str().unwrap(),
        "--clock",
        "10",
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("关键路径 4"), "{stdout}");
    assert!(stdout.contains("slack=6"), "{stdout}");
}

#[test]
fn timing_lists_critical_paths() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chain3.net");
    let (stdout, stderr, code) = run(&[
        "timing",
        path.to_str().unwrap(),
        "--paths",
        "2",
        "--clock",
        "10",
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("#1 delay=2"), "{stdout}");
    assert!(stdout.contains("#2 delay=2"), "{stdout}");
    assert!(stdout.contains(" -> y"), "{stdout}");
}

#[test]
fn check_fails_on_timing_violation() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chain3.net");
    let (stdout, _, code) = run(&["check", path.to_str().unwrap(), "--clock", "1"]);
    assert_eq!(code, 1, "{stdout}");
    assert!(stdout.contains("FAIL"), "{stdout}");
    assert!(stdout.contains("时序违例 1"), "{stdout}");
}

#[test]
fn check_passes_with_slack_and_coverage() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chain3.net");
    let (stdout, stderr, code) = run(&[
        "check",
        path.to_str().unwrap(),
        "--clock",
        "10",
        "--min-coverage",
        "0.5",
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("PASS"), "{stdout}");
    assert!(stdout.contains("ATPG：覆盖率 100.0%"), "{stdout}");
}

#[test]
fn timing_uses_load_delay() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fanout3.net");
    let (stdout, stderr, code) = run(&[
        "timing",
        path.to_str().unwrap(),
        "--load-delay",
        "2",
        "--clock",
        "8",
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("负载模型：每扇出 +2 周期"), "{stdout}");
    assert!(stdout.contains("关键路径 8 周期"), "{stdout}");
    assert!(stdout.contains("违例 0 个"), "{stdout}");
}

#[test]
fn map_load_delay_prefers_direct_cell() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let net = dir.join("and2.net");
    let cells = dir.join("cells_and3.json");
    let net = net.to_str().unwrap();
    let cells = cells.to_str().unwrap();

    let (stdout, stderr, code) = run(&["map", net, "--cells", cells]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert_eq!(stdout.matches("gate NAND").count(), 2, "{stdout}");

    let (stdout, stderr, code) = run(&["map", net, "--cells", cells, "--load-delay", "2"]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("gate AND"), "{stdout}");
    assert!(!stdout.contains("gate NAND"), "{stdout}");
}

#[test]
fn netlist_refuses_to_export_strong_xor() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/strong_xor.net");
    let (stdout, stderr, code) = run(&["netlist", path.to_str().unwrap(), "--to", "verilog"]);
    assert_eq!(code, 1, "stdout={stdout} stderr={stderr}");
    assert!(stderr.contains("强加 Xor"), "stderr={stderr}");
}

#[test]
fn timing_overflow_is_an_error() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fanout3.net");
    let (stdout, stderr, code) = run(&[
        "timing",
        path.to_str().unwrap(),
        "--delay",
        "4294967295",
        "--clock",
        "4294967295",
    ]);
    assert_eq!(code, 1, "stdout={stdout} stderr={stderr}");
    assert!(stderr.contains("超出 u32"), "stderr={stderr}");
}

#[test]
fn check_fails_closed_when_coverage_cannot_run() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wide11.net");
    let path = path.to_str().unwrap();

    let (stdout, stderr, code) = run(&["check", path, "--clock", "999", "--min-coverage", "0.9"]);
    assert_eq!(code, 1, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("覆盖率检查未执行"), "{stdout}");
    assert!(stdout.contains("结果：FAIL"), "{stdout}");

    let (stdout, stderr, code) = run(&["check", path, "--clock", "999"]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("ATPG：跳过"), "{stdout}");

    let (_, stderr, code) = run(&["check", path, "--min-coverage", "1.5"]);
    assert_eq!(code, 1, "stderr={stderr}");
    assert!(stderr.contains("--min-coverage"), "{stderr}");
}

#[test]
fn netlist_parses_and_writes_aiger() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let aiger = dir.join("and2.aag");
    let net = dir.join("and2.net");
    let (stdout, stderr, code) = run(&[
        "netlist",
        aiger.to_str().unwrap(),
        "--from",
        "aiger",
        "--to",
        "text",
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("gate AND"), "{stdout}");

    let (stdout, stderr, code) = run(&["netlist", net.to_str().unwrap(), "--to", "aiger"]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.starts_with("aag "), "{stdout}");
    assert!(!stdout.contains(".end"), "{stdout}");

    let (stdout, stderr, code) = run(&["atpg", aiger.to_str().unwrap(), "--from", "aiger"]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("覆盖率 100.0%"), "{stdout}");
}

#[test]
fn netlist_parses_and_writes_blif() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let blif = dir.join("and2.blif");
    let net = dir.join("and2.net");
    let (stdout, stderr, code) = run(&[
        "netlist",
        blif.to_str().unwrap(),
        "--from",
        "blif",
        "--to",
        "text",
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("gate AND"), "{stdout}");

    let (stdout, stderr, code) = run(&["netlist", net.to_str().unwrap(), "--to", "blif"]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains(".inputs a b"), "{stdout}");
    assert!(stdout.contains(".names a b y"), "{stdout}");
    assert!(stdout.contains(".end"), "{stdout}");

    let (stdout, stderr, code) = run(&["atpg", blif.to_str().unwrap(), "--from", "blif"]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("覆盖率 100.0%"), "{stdout}");
}

#[test]
fn netlist_parses_verilog_assign_xor() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/xor_assign.v");
    let (stdout, stderr, code) = run(&[
        "netlist",
        path.to_str().unwrap(),
        "--from",
        "verilog",
        "--to",
        "text",
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("gate XOR2"), "{stdout}");
}

#[test]
fn map_lowers_to_nand_only() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/and2.net");
    let (stdout, stderr, code) = run(&["map", path.to_str().unwrap()]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("gate NAND"), "{stdout}");
    assert!(!stdout.contains("gate AND"), "{stdout}");
}

#[test]
fn map_cell_library_picks_implementation_by_timing() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let net = dir.join("and2.net");
    let net = net.to_str().unwrap();

    // AND 延迟 1 < 两级 NAND 延迟 8：时序驱动应保留单个 AND 单元。
    let cheap_and = dir.join("cells_and.json");
    let (stdout, stderr, code) = run(&["map", net, "--cells", cheap_and.to_str().unwrap()]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("gate AND"), "{stdout}");
    assert!(!stdout.contains("gate NAND"), "{stdout}");

    // AND 延迟 4 > 两级 NAND 延迟 2：时序驱动应展开成 NAND。
    let cheap_nand = dir.join("cells_nand.json");
    let (stdout, stderr, code) = run(&["map", net, "--cells", cheap_nand.to_str().unwrap()]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(!stdout.contains("gate AND"), "{stdout}");
    assert_eq!(stdout.matches("gate NAND").count(), 2, "{stdout}");

    // 面积优先同库下选单个 AND（1 个单元），哪怕更慢。
    let (stdout, stderr, code) = run(&[
        "map",
        net,
        "--cells",
        cheap_nand.to_str().unwrap(),
        "--area-first",
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("gate AND"), "{stdout}");
}

#[test]
fn map_area_first_without_cells_is_rejected() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/and2.net");
    let (_, stderr, code) = run(&["map", path.to_str().unwrap(), "--area-first"]);
    assert_eq!(code, 1);
    assert!(stderr.contains("--area-first 需要配合 --cells"), "{stderr}");
}

#[test]
fn atpg_sat_supports_more_than_ten_inputs() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/and20.net");
    let (stdout, stderr, code) = run(&["atpg", path.to_str().unwrap(), "--sat"]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("SAT 逐故障"), "{stdout}");
    assert!(stdout.contains("覆盖率 100.0%"), "{stdout}");

    let (_, stderr, code) = run(&["atpg", path.to_str().unwrap()]);
    assert_ne!(code, 0);
    assert!(stderr.contains("可用 --sat"), "{stderr}");
}

#[test]
fn bridge_sat_matches_small_fixture() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let netlist = dir.join("and2.net");
    let pairs = dir.join("bridges.json");
    let (stdout, stderr, code) = run(&[
        "bridge",
        netlist.to_str().unwrap(),
        "--pairs",
        pairs.to_str().unwrap(),
        "--sat",
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("SAT 逐桥接"), "{stdout}");
    assert!(stdout.contains("覆盖率 50.0%"), "{stdout}");
}

#[test]
fn timing_constraints_shift_arrival_and_slack() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let netlist = dir.join("and2.net");
    let constraints = dir.join("timing_constraints.json");
    let (stdout, stderr, code) = run(&[
        "timing",
        netlist.to_str().unwrap(),
        "--constraints",
        constraints.to_str().unwrap(),
        "--format",
        "json",
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("\"arrival\": 3"), "{stdout}");
    assert!(stdout.contains("\"required\": 5"), "{stdout}");
    assert!(stdout.contains("\"slack\": 2"), "{stdout}");

    let (stdout, stderr, code) = run(&[
        "check",
        netlist.to_str().unwrap(),
        "--constraints",
        constraints.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
}

#[test]
fn jobs_do_not_change_atpg_or_bridge_output() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let netlist = dir.join("and2.net");
    let path = netlist.to_str().unwrap();

    let serial = run(&["atpg", path, "--sat", "--jobs", "1", "--format", "json"]);
    let parallel = run(&["atpg", path, "--sat", "--jobs", "4", "--format", "json"]);
    assert_eq!(serial.2, 0, "stdout={} stderr={}", serial.0, serial.1);
    assert_eq!(parallel.2, 0, "stdout={} stderr={}", parallel.0, parallel.1);
    assert_eq!(serial.0, parallel.0);

    let serial = run(&[
        "bridge",
        path,
        "--sat",
        "--max-pairs",
        "8",
        "--jobs",
        "1",
        "--format",
        "json",
    ]);
    let parallel = run(&[
        "bridge",
        path,
        "--sat",
        "--max-pairs",
        "8",
        "--jobs",
        "4",
        "--format",
        "json",
    ]);
    assert_eq!(serial.2, 0, "stdout={} stderr={}", serial.0, serial.1);
    assert_eq!(parallel.2, 0, "stdout={} stderr={}", parallel.0, parallel.1);
    assert_eq!(serial.0, parallel.0);
}

#[test]
fn sdf_delays_feed_timing_and_check() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let netlist = dir.join("and2.net");
    let sdf = dir.join("and2.sdf");
    let (stdout, stderr, code) = run(&[
        "timing",
        netlist.to_str().unwrap(),
        "--sdf",
        sdf.to_str().unwrap(),
        "--format",
        "json",
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("\"critical_path\": 3"), "{stdout}");
    assert!(stdout.contains("\"arrival\": 3"), "{stdout}");

    let (stdout, stderr, code) = run(&[
        "check",
        netlist.to_str().unwrap(),
        "--sdf",
        sdf.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");

    let (_, _, code) = run(&[
        "timing",
        netlist.to_str().unwrap(),
        "--sdf",
        sdf.to_str().unwrap(),
        "--library",
        dir.join("timing_constraints.json").to_str().unwrap(),
    ]);
    assert_ne!(code, 0, "--sdf 与 --library 必须互斥");
}

#[test]
fn verify_jobs_match_serial_output() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let policy = dir.join("admission.json");
    let path = policy.to_str().unwrap();
    let serial = run(&["verify", path, "--json"]);
    let parallel = run(&["verify", path, "--json", "--jobs", "4"]);
    assert_eq!(serial.2, 0, "stdout={} stderr={}", serial.0, serial.1);
    assert_eq!(parallel.2, 0, "stdout={} stderr={}", parallel.0, parallel.1);
    assert_eq!(serial.0, parallel.0);
}

#[test]
fn shared_cnf_atpg_matches_coverage() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let netlist = dir.join("and2.net");
    let path = netlist.to_str().unwrap();
    let default = run(&["atpg", path, "--sat", "--jobs", "1", "--format", "json"]);
    let shared = run(&[
        "atpg",
        path,
        "--sat",
        "--shared-cnf",
        "--jobs",
        "4",
        "--format",
        "json",
    ]);
    assert_eq!(default.2, 0, "stdout={} stderr={}", default.0, default.1);
    assert_eq!(shared.2, 0, "stdout={} stderr={}", shared.0, shared.1);
    let default_json: serde_json::Value = serde_json::from_str(&default.0).unwrap();
    let shared_json: serde_json::Value = serde_json::from_str(&shared.0).unwrap();
    assert_eq!(default_json["testable"], shared_json["testable"]);
    assert_eq!(default_json["coverage"], shared_json["coverage"]);
    assert_eq!(shared_json["shared_cnf"], true);
}

#[test]
fn verilog_parameters_expand_before_netlist_export() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let path = dir.join("params.v");
    let (stdout, stderr, code) = run(&[
        "netlist",
        path.to_str().unwrap(),
        "--from",
        "verilog",
        "--to",
        "json",
    ]);
    assert_eq!(code, 0, "stderr={stderr}");
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let buses = json["buses"].as_object().unwrap();
    assert_eq!(buses["y"]["width"], 3, "{json}");
}

#[test]
fn timeout_kills_long_running_command_with_124() {
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let mut child = Command::new(trit_bin())
        .args(["repl", "--timeout-ms", "200"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn trit");
    let _stdin = child.stdin.take().expect("stdin pipe");
    let started = Instant::now();
    let deadline = started + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("timeout wrapper did not terminate the child");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(124));
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn sdf_per_instance_delays_reach_each_output() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let netlist = dir.join("two_leaves.v");
    let sdf = dir.join("two_leaves.sdf");
    let (stdout, stderr, code) = run(&[
        "timing",
        netlist.to_str().unwrap(),
        "--from",
        "verilog",
        "--sdf",
        sdf.to_str().unwrap(),
        "--format",
        "json",
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("\"name\": \"y1\""), "{stdout}");
    assert!(stdout.contains("\"arrival\": 2"), "{stdout}");
    assert!(stdout.contains("\"name\": \"y2\""), "{stdout}");
    assert!(stdout.contains("\"arrival\": 5"), "{stdout}");
    assert!(stdout.contains("\"critical_path\": 5"), "{stdout}");
}

#[test]
fn schedule_command_handles_unknown_dependencies() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let path = dir.join("schedule.json");
    let (stdout, stderr, code) = run(&[
        "schedule",
        path.to_str().unwrap(),
        "--workers",
        "2",
        "--format",
        "json",
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["makespan"], 7);
    assert_eq!(report["critical_path"], 7);
    assert_eq!(report["scheduled"].as_array().unwrap().len(), 3);
    assert_eq!(report["deferred"][0]["id"], "c");
    assert_eq!(
        report["deferred"][0]["reason"]["kind"],
        "unknown_dependency"
    );
}
