use super::*;

fn process(pid: u32, name: &str, cmd: &str, cpu_ms: u64) -> Event {
    Event::Process {
        ts_ms: 1000 + pid as u64,
        pid,
        ppid: 1,
        name: name.to_string(),
        command_line: cmd.to_string(),
        working_set: 0,
        cpu_ms,
    }
}

#[test]
fn flags_shell_network_and_encoded_commands() {
    let events = vec![
        process(1, "app.exe", "app.exe", 10),
        process(2, "cmd.exe", "cmd.exe /c whoami", 5),
        process(3, "curl.exe", "curl http://x", 5),
        process(
            4,
            "powershell.exe",
            "powershell -EncodedCommand SQBFAFgA",
            5,
        ),
    ];
    let report = analyze(&events).unwrap();
    let ids: Vec<&str> = report.findings.iter().map(|finding| finding.id).collect();
    for expected in [
        "spawns-shell",
        "spawns-network-tool",
        "encoded-command-line",
    ] {
        assert!(ids.contains(&expected), "{ids:?}");
    }
    assert!(has_findings(&report));
    assert_eq!(report.processes, 4);
}

#[test]
fn loopback_connections_are_not_flagged_but_public_ones_are() {
    let events = vec![
        Event::Connection {
            ts_ms: 1,
            pid: 1,
            remote: "127.0.0.1:3425".into(),
            state: "Established".into(),
        },
        Event::Connection {
            ts_ms: 2,
            pid: 1,
            remote: "10.1.2.3:443".into(),
            state: "Established".into(),
        },
        Event::Connection {
            ts_ms: 3,
            pid: 1,
            remote: "93.184.216.34:443".into(),
            state: "Established".into(),
        },
    ];
    let report = analyze(&events).unwrap();
    let outbound = report
        .findings
        .iter()
        .find(|finding| finding.id == "outbound-connection")
        .expect("必须报外联");
    assert_eq!(outbound.evidence.len(), 1, "{outbound:?}");
    assert!(outbound.evidence[0].contains("93.184.216.34"));
}

#[test]
fn temp_modules_and_executable_writes_are_flagged() {
    let events = vec![
        Event::Module {
            ts_ms: 1,
            pid: 9,
            process: "x.exe".into(),
            path: "C:\\Users\\u\\AppData\\Local\\Temp\\a.dll".into(),
            exports: None,
        },
        Event::FileWrite {
            ts_ms: 2,
            path: "out\\dropper.exe".into(),
            bytes: 12,
        },
    ];
    let report = analyze(&events).unwrap();
    let ids: Vec<&str> = report.findings.iter().map(|finding| finding.id).collect();
    assert!(ids.contains(&"loads-module-from-temp"), "{ids:?}");
    assert!(ids.contains(&"writes-executable"), "{ids:?}");
    assert_eq!(report.writes, 1);
}

#[test]
fn cpu_ms_accumulation_is_exact_for_legal_inputs() {
    // 回归：合法路径下累计值与修前的普通加法逐位一致；零值不贡献。
    let events = vec![
        process(1, "worker", "worker", 100),
        process(2, "worker", "worker", 200),
        process(3, "idle", "idle", 0),
        process(4, "worker", "worker", 50),
    ];
    let report = analyze(&events).unwrap();
    assert_eq!(report.top_cpu, vec![("worker".to_string(), 350)]);
}

#[test]
fn zero_cpu_ms_and_duplicate_records_add_nothing() {
    let events = vec![
        process(1, "loop", "loop", 0),
        process(2, "loop", "loop", 0),
        process(3, "loop", "loop", 42),
        process(4, "loop", "loop", 0),
    ];
    let report = analyze(&events).unwrap();
    assert_eq!(report.top_cpu, vec![("loop".to_string(), 42)]);
}

#[test]
fn values_near_u32_max_accumulate_exactly_in_u64() {
    // 两条接近 u32::MAX 的记录在 u64 内可精确相加（不会误判溢出）。
    let events = vec![
        process(1, "big", "big", u32::MAX as u64),
        process(2, "big", "big", u32::MAX as u64),
    ];
    let report = analyze(&events).unwrap();
    assert_eq!(
        report.top_cpu,
        vec![("big".to_string(), 2 * (u32::MAX as u64))]
    );
}

#[test]
fn cpu_overflow_returns_structured_error_instead_of_wrapping() {
    // 极端：两条各 15e18（接近 u64::MAX）同名进程，和超出 u64。
    // 修前 release 静默回绕成小错值、debug panic；修后必须返回结构化错误。
    let events = vec![
        process(1, "big", "x", 15_000_000_000_000_000_000),
        process(2, "big", "x", 15_000_000_000_000_000_000),
    ];
    let err = analyze(&events).unwrap_err();
    match err {
        AnalyzeError::CpuTimeOverflow {
            process,
            accumulated,
            additional,
        } => {
            assert_eq!(process, "big");
            assert_eq!(accumulated, 15_000_000_000_000_000_000);
            assert_eq!(additional, 15_000_000_000_000_000_000);
        }
    }
}

#[test]
fn boundary_at_u64_max_then_one_more_overflows() {
    // 临界：MAX - 10 + 10 恰好等于 MAX（合法，逐位保持）；再加 1 即溢出。
    let events = vec![
        process(1, "crit", "x", u64::MAX - 10),
        process(2, "crit", "x", 10),
    ];
    let report = analyze(&events).unwrap();
    assert_eq!(report.top_cpu, vec![("crit".to_string(), u64::MAX)]);

    let events = vec![
        process(1, "crit", "x", u64::MAX),
        process(2, "crit", "x", 1),
    ];
    let err = analyze(&events).unwrap_err();
    match err {
        AnalyzeError::CpuTimeOverflow {
            process,
            accumulated,
            additional,
        } => {
            assert_eq!(process, "crit");
            assert_eq!(accumulated, u64::MAX);
            assert_eq!(additional, 1);
        }
    }
}
