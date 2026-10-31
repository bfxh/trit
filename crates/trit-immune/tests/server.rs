#![cfg(feature = "full")]
//! 本地 HTTP 服务器：端点、扫描入账、双服务器 HTTP 同步收敛、并发读写。

use std::net::SocketAddr;
use trit_immune::*;

fn temp(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("trit-immune-{name}-{}.journal", std::process::id()))
}

fn read_log(path: &std::path::Path) -> Vec<Stamped> {
    sync::parse_log_file(path).unwrap()
}

fn bind_server(journal: Journal) -> SocketAddr {
    let server = Server::new(journal);
    server
        .serve("127.0.0.1:0".parse().unwrap())
        .expect("起服务器")
}

#[test]
fn endpoints_basic_and_scan_ingest() {
    let pa = temp("srv-basic");
    let _ = std::fs::remove_file(&pa);
    let mut j = Journal::create_as(&pa, "main", 0).unwrap();
    let cfg = j.spawn(NodeId(0), NodeKind::Delta, "配置").unwrap();
    j.record(cfg, Trit::Hardened, None, "通过").unwrap();
    j.into_dag().unwrap();

    let addr = bind_server(Journal::open_as(&pa, 0).unwrap());

    // 健康与只读面
    let (s, b) = server::client::get(addr, "/v1/health").unwrap();
    assert_eq!((s, b.trim()), (200, "ok"));
    let (s, b) = server::client::get(addr, "/v1/render").unwrap();
    assert_eq!(s, 200);
    assert!(b.contains("#01 DELTA"));
    let (s, b) = server::client::get(addr, "/v1/gate?tip=1").unwrap();
    assert_eq!(s, 200);
    assert!(b.contains("gate=ALLOW"), "扫描前应放行：{b}");
    let (s, b) = server::client::get(addr, "/v1/suspects").unwrap();
    assert_eq!(s, 200);
    assert!(b.trim().is_empty(), "无 0 态节点");

    // 边界：未知节点 / 未知路径 / 错误方法
    assert_eq!(server::client::get(addr, "/v1/gate?tip=99").unwrap().0, 404);
    assert_eq!(server::client::get(addr, "/v1/nope").unwrap().0, 404);
    assert_eq!(
        server::client::request(addr, "DELETE", "/v1/health", "")
            .unwrap()
            .0,
        405
    );

    // 扫描入账：硬编码凭据（Dispositive）→ -1 → 门禁 BLOCK
    let body = "SNIP\t1\tdb.password = \"hunter2\";";
    let (s, b) = server::client::post(addr, "/v1/scan", body).unwrap();
    assert_eq!(s, 200);
    assert!(b.contains("applied=1"), "{b}");
    assert!(b.contains("scanner_never_plus1=true"));
    let (s, b) = server::client::get(addr, "/v1/gate?tip=1").unwrap();
    assert_eq!(s, 200);
    assert!(b.contains("gate=BLOCK"), "扫描证伪后应 BLOCK：{b}");

    // 启发式 → 0 → 进待办队列
    let (s, _) = server::client::post(addr, "/v1/scan", "SNIP\t1\tel.innerHTML = userInput;").unwrap();
    assert_eq!(s, 200);
    let (s, b) = server::client::get(addr, "/v1/suspects").unwrap();
    assert_eq!(s, 200);
    assert!(b.contains("配置"), "0 态节点应入队：{b}");

    let _ = std::fs::remove_file(&pa);
}

#[test]
fn http_sync_converges_two_servers() {
    let pa = temp("srv-sync-a");
    let pb = temp("srv-sync-b");
    let _ = std::fs::remove_file(&pa);
    let _ = std::fs::remove_file(&pb);

    // A 建库
    let mut a = Journal::create_as(&pa, "main", 0).unwrap();
    let login = a.spawn(NodeId(0), NodeKind::Delta, "登录").unwrap();
    let atk = a.spawn(login, NodeKind::AttackVector, "SQLi").unwrap();
    a.record(atk, Trit::Vulnerable, None, "打穿").unwrap();
    a.into_dag().unwrap();
    // B = 克隆 + 追加
    std::fs::copy(&pa, &pb).unwrap();
    let mut b = Journal::open_as(&pb, 1).unwrap();
    let pay = b.spawn(NodeId(0), NodeKind::Delta, "支付").unwrap();
    b.record(pay, Trit::Hardened, None, "通过").unwrap();
    b.into_dag().unwrap();

    let addr_a = bind_server(Journal::open_as(&pa, 0).unwrap());
    let addr_b = bind_server(Journal::open_as(&pb, 1).unwrap());

    // A 的帧 POST 给 B：B 合并采纳
    let frame_a = sync::encode_frame(0, &read_log(&pa));
    let (s, b) = server::client::post(addr_b, "/v1/deltas", &frame_a).unwrap();
    assert_eq!(s, 200);
    assert!(b.contains("canonical=6"), "去重后 union 应为 6 条：{b}");
    assert!(b.contains("deferred=0"));

    // B 的帧 POST 给 A：A 合并采纳 → 双方收敛
    let frame_b = sync::encode_frame(1, &read_log(&pb));
    let (s, b) = server::client::post(addr_a, "/v1/deltas", &frame_b).unwrap();
    assert_eq!(s, 200);
    assert!(b.contains("canonical=6"));

    // 双方日志逐字节一致，节点数一致
    let la = std::fs::read_to_string(&pa).unwrap();
    let lb = std::fs::read_to_string(&pb).unwrap();
    assert_eq!(la, lb, "HTTP 同步后双方日志应逐字节一致");

    let _ = std::fs::remove_file(&pa);
    let _ = std::fs::remove_file(&pb);
}

#[test]
fn diag_endpoint_two_states_and_garbage_guard() {
    // 编译器管线桥：rustc 诊断流 → 层裁决（检查类可为 +1，扫描类永不）
    let pa = temp("srv-diag");
    let _ = std::fs::remove_file(&pa);
    let mut j = Journal::create_as(&pa, "main", 0).unwrap();
    let _ = j.spawn(NodeId(0), NodeKind::Delta, "功能").unwrap();
    j.into_dag().unwrap();

    let addr = bind_server(Journal::open_as(&pa, 0).unwrap());

    // 垃圾输入 → 400（不能让空 POST 把节点洗成 +1）
    let (s, _) = server::client::post(addr, "/v1/diag?node=1", "").unwrap();
    assert_eq!(s, 400, "空体应拒绝");
    let (s, _) = server::client::post(addr, "/v1/diag?node=1", "纯文本垃圾").unwrap();
    assert_eq!(s, 400, "无 JSON 应拒绝");

    // 错误流：error 诊断 → 层 -1 → 门禁 BLOCK
    let err_stream = concat!(
        r#"{"reason":"compiler-message","message":{"level":"error","message":"mismatched types","code":{"code":"E0308"},"spans":[{"file_name":"src/main.rs","line_start":3}]}}"#,
        "\n",
        r#"{"reason":"build-finished","success":false}"#,
        "\n"
    );
    let (s, b) = server::client::post(addr, "/v1/diag?node=1", err_stream).unwrap();
    assert_eq!(s, 200);
    assert!(b.contains("errors=1"), "{b}");
    assert!(b.contains("layer=-1"), "{b}");
    let (_, b) = server::client::get(addr, "/v1/gate?tip=1").unwrap();
    assert!(b.contains("gate=BLOCK"), "构建失败应 BLOCK：{b}");

    // 干净构建（有 build-finished，0 诊断）→ 检查类 +1 → 门禁放行
    let clean_stream = r#"{"reason":"build-finished","success":true}"#;
    let (_, b) = server::client::post(addr, "/v1/diag?node=1", clean_stream).unwrap();
    assert!(b.contains("layer=+1"), "{b}");
    let (_, b) = server::client::get(addr, "/v1/gate?tip=1").unwrap();
    assert!(b.contains("gate=ALLOW"), "干净构建应放行：{b}");

    let _ = std::fs::remove_file(&pa);
}

#[test]
fn lsp_endpoint_two_states() {
    // LSP 桥：publishDiagnostics 流 → 层裁决（检查类语义，与 /v1/diag 对称）
    let pa = temp("srv-lsp");
    let _ = std::fs::remove_file(&pa);
    let mut j = Journal::create_as(&pa, "main", 0).unwrap();
    let _ = j.spawn(NodeId(0), NodeKind::Delta, "功能").unwrap();
    j.into_dag().unwrap();

    let addr = bind_server(Journal::open_as(&pa, 0).unwrap());

    // 错误诊断 → 层 -1 → BLOCK
    let err_stream = concat!(
        r#"{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":"file:///src/main.rs","diagnostics":[{"range":{"start":{"line":2}},"severity":1,"code":"E0308","source":"rust-analyzer","message":"mismatched types"}]}}"#,
        "\n"
    );
    let (s, b) = server::client::post(addr, "/v1/lsp?node=1", err_stream).unwrap();
    assert_eq!(s, 200);
    assert!(b.contains("errors=1"), "{b}");
    assert!(b.contains("layer=-1"), "{b}");
    let (_, b) = server::client::get(addr, "/v1/gate?tip=1").unwrap();
    assert!(b.contains("gate=BLOCK"), "LSP 错误应 BLOCK：{b}");

    // 仅 hint（缺 severity）→ Note → 干净 → 检查类 +1 → 放行
    let clean_stream = concat!(
        r#"{"params":{"uri":"file:///src/main.rs","diagnostics":[{"range":{"start":{"line":0}},"message":"hint"}]}}"#,
        "\n",
        r#"{"reason":"build-finished","success":true}"#,
        "\n"
    );
    let (s, b) = server::client::post(addr, "/v1/lsp?node=1", clean_stream).unwrap();
    assert_eq!(s, 200);
    assert!(b.contains("layer=+1"), "{b}");
    let (_, b) = server::client::get(addr, "/v1/gate?tip=1").unwrap();
    assert!(b.contains("gate=ALLOW"), "干净应放行：{b}");

    let _ = std::fs::remove_file(&pa);
}

#[test]
fn server_handles_concurrent_readers_and_writers() {
    let pa = temp("srv-conc");
    let _ = std::fs::remove_file(&pa);
    let mut j = Journal::create_as(&pa, "main", 0).unwrap();
    let n1 = j.spawn(NodeId(0), NodeKind::Delta, "模块一").unwrap();
    j.record(n1, Trit::Hardened, None, "通过").unwrap();
    let n2 = j.spawn(NodeId(0), NodeKind::Delta, "模块二").unwrap();
    j.record(n2, Trit::Hardened, None, "通过").unwrap();
    j.into_dag().unwrap();

    let addr = bind_server(Journal::open_as(&pa, 0).unwrap());

    // 4 个读者线程 × 15 个请求，与写者并发
    let mut readers = Vec::new();
    for t in 0..4u32 {
        let addr = addr;
        readers.push(std::thread::spawn(move || {
            for i in 0..15u32 {
                let path = match (t + i) % 3 {
                    0 => "/v1/render",
                    1 => "/v1/gate?tip=1",
                    _ => "/v1/suspects",
                };
                let (s, _) = server::client::get(addr, path).unwrap();
                assert_eq!(s, 200, "读者 {t} 请求 {path} 失败");
            }
        }));
    }

    // 写者：两次扫描入账（一次确证 / 一次启发式）+ 一次自帧合并（幂等）
    let (s, _) = server::client::post(addr, "/v1/scan", "SNIP\t1\tsecret = \"k\";").unwrap();
    assert_eq!(s, 200);
    let (s, _) = server::client::post(addr, "/v1/scan", "SNIP\t2\tSELECT * FROM t\" + user;").unwrap();
    assert_eq!(s, 200);
    let frame = sync::encode_frame(0, &sync::parse_log_file(&pa).unwrap());
    let (s, _) = server::client::post(addr, "/v1/deltas", &frame).unwrap();
    assert_eq!(s, 200, "自帧合并应幂等成功");

    for r in readers {
        r.join().expect("读者线程");
    }

    // 确证规则 → BLOCK；启发式规则 → FREEZE（0 态进红队队列）
    let (s, b) = server::client::get(addr, "/v1/gate?tip=1").unwrap();
    assert_eq!(s, 200);
    assert!(b.contains("gate=BLOCK"));
    let (s, b) = server::client::get(addr, "/v1/gate?tip=2").unwrap();
    assert_eq!(s, 200);
    assert!(b.contains("gate=FREEZE"), "启发式应冻结：{b}");

    let _ = std::fs::remove_file(&pa);
}
