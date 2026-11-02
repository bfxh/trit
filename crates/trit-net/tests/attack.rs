//! 三值攻击图（M2）的语义测试：折叠走 K3 AND、跨路径 K3 OR、截断封顶、
//! 台账往返、登记纪律。全部合成夹具，不碰网络。

use trit_core::value::Ternary;
use trit_net::netsec::attack::{
    AttackError, AttackGraph, Evidence, Limits, OpenStep, PathDetail, Reach,
};

fn graph() -> AttackGraph {
    let mut graph = AttackGraph::new();
    let entry = graph.add_node("entry", Evidence::Confirmed, "已确认立足点");
    let dmz = graph.add_node("dmz", Evidence::Confirmed, "");
    let core = graph.add_node("core", Evidence::Confirmed, "");
    graph
        .add_edge(entry, dmz, Evidence::Confirmed, "初始访问：已复现利用")
        .unwrap();
    graph
        .add_edge(dmz, core, Evidence::Confirmed, "横向移动：已复现利用")
        .unwrap();
    let _ = core;
    graph
}

#[test]
fn proven_path_requires_every_hop_confirmed() {
    let graph = graph();
    let entry = graph.node_id("entry").unwrap();
    let core = graph.node_id("core").unwrap();
    let reach = graph.reach(entry, core, Limits::default()).unwrap();
    assert_eq!(reach.verdict, Evidence::Confirmed);
    assert_eq!(reach.reason, "proven-path");
    assert!(!reach.truncated);
    assert_eq!(reach.paths.len(), 1);
    assert!(reach.paths[0].open.is_empty());
}

#[test]
fn one_unverified_edge_caps_the_whole_path() {
    let mut graph = graph();
    let entry = graph.node_id("entry").unwrap();
    let dmz = graph.node_id("dmz").unwrap();
    let core = graph.node_id("core").unwrap();
    // 核心里的横向移动只是声称，还没验证。
    graph
        .add_edge(dmz, core, Evidence::Unverified, "内网扫描报告，待复现")
        .unwrap();
    let reach = graph.reach(entry, core, Limits::default()).unwrap();
    assert_eq!(reach.verdict, Evidence::Unverified);
    assert_eq!(reach.reason, "unverified-only");
    // 见证：必须点名那条待验证的边，不能让人猜。
    assert_eq!(
        reach.paths[0].open,
        vec![OpenStep {
            node: dmz,
            edge_to: Some(core),
        }]
    );
}

#[test]
fn unverified_entry_node_also_caps_the_path() {
    let mut graph = AttackGraph::new();
    let entry = graph.add_node("claim", Evidence::Unverified, " agent 自称立足点");
    let goal = graph.add_node("goal", Evidence::Confirmed, "");
    graph
        .add_edge(entry, goal, Evidence::Confirmed, "任意一步")
        .unwrap();
    let reach = graph.reach(entry, goal, Limits::default()).unwrap();
    assert_eq!(reach.verdict, Evidence::Unverified);
    assert_eq!(
        reach.paths[0].open,
        vec![OpenStep {
            node: entry,
            edge_to: None,
        }]
    );
}

#[test]
fn ruled_out_edge_blocks_only_its_own_route() {
    let mut graph = AttackGraph::new();
    let entry = graph.add_node("entry", Evidence::Confirmed, "");
    let goal = graph.add_node("goal", Evidence::Confirmed, "");
    let vpn = graph.add_node("vpn", Evidence::Confirmed, "");
    let printer = graph.add_node("printer", Evidence::Confirmed, "");
    // 路线 A（经 VPN）：已证实。
    graph
        .add_edge(entry, vpn, Evidence::Confirmed, "路线A第一跳")
        .unwrap();
    graph
        .add_edge(vpn, goal, Evidence::Confirmed, "路线A第二跳")
        .unwrap();
    // 路线 B（经打印机）：第一跳已被打补丁排除。
    graph
        .add_edge(entry, printer, Evidence::Confirmed, "路线B第一跳")
        .unwrap();
    graph
        .add_edge(printer, goal, Evidence::RuledOut, "补丁已装，排除")
        .unwrap();
    let reach = graph.reach(entry, goal, Limits::default()).unwrap();
    // 另一条路线仍证实 ⇒ 总判决不降；被排除的路线不出现在已证实明细里。
    assert_eq!(reach.verdict, Evidence::Confirmed);
    assert_eq!(reach.paths.len(), 2);
    let proven: Vec<&PathDetail> = reach
        .paths
        .iter()
        .filter(|path| path.fold == Evidence::Confirmed)
        .collect();
    assert_eq!(proven.len(), 1);
    assert!(proven[0].nodes.contains(&vpn));
    assert!(!proven[0].nodes.contains(&printer));
    // 把路线 A 也排除 ⇒ 所有路线都排除。
    graph
        .add_edge(entry, vpn, Evidence::RuledOut, "VPN 已下线")
        .unwrap();
    graph
        .add_edge(vpn, goal, Evidence::RuledOut, "随之下线")
        .unwrap();
    let reach = graph.reach(entry, goal, Limits::default()).unwrap();
    assert_eq!(reach.verdict, Evidence::RuledOut);
    assert_eq!(reach.reason, "all-ruled-out");
}

#[test]
fn no_path_is_in_graph_ruled_out_not_absolute() {
    let mut graph = AttackGraph::new();
    let a = graph.add_node("a", Evidence::Confirmed, "");
    let b = graph.add_node("b", Evidence::Confirmed, "");
    let reach = graph.reach(a, b, Limits::default()).unwrap();
    assert_eq!(reach.verdict, Evidence::RuledOut);
    assert_eq!(reach.reason, "no-path");
    assert!(reach.paths.is_empty());
}

#[test]
fn self_reach_degenerates_to_node_evidence() {
    let mut graph = AttackGraph::new();
    let x = graph.add_node("x", Evidence::Unverified, "");
    let reach = graph.reach(x, x, Limits::default()).unwrap();
    assert_eq!(reach.verdict, Evidence::Unverified);
    assert_eq!(reach.reason, "self");
    assert_eq!(reach.paths[0].nodes, vec![x]);
}

#[test]
fn cycles_terminate_and_do_not_confuse_the_verdict() {
    let mut graph = AttackGraph::new();
    let a = graph.add_node("a", Evidence::Confirmed, "");
    let b = graph.add_node("b", Evidence::Confirmed, "");
    let c = graph.add_node("c", Evidence::Confirmed, "");
    graph.add_edge(a, b, Evidence::Confirmed, "ab").unwrap();
    graph.add_edge(b, a, Evidence::Confirmed, "ba").unwrap(); // 环
    graph.add_edge(b, c, Evidence::Confirmed, "bc").unwrap();
    graph.add_edge(a, a, Evidence::Confirmed, "自环").unwrap();
    let reach = graph.reach(a, c, Limits::default()).unwrap();
    assert_eq!(reach.verdict, Evidence::Confirmed);
    assert!(!reach.truncated);
}

#[test]
fn truncation_never_confirms_and_never_rules_out() {
    // 一条深链，深度预算只够走一半 ⇒ 枚举不全 ⇒ 一律待验证。
    let mut graph = AttackGraph::new();
    let mut prev = graph.add_node("n0", Evidence::Confirmed, "");
    let first = prev;
    let mut last = prev;
    for i in 1..6 {
        let node = graph.add_node(&format!("n{i}"), Evidence::Confirmed, "");
        graph
            .add_edge(prev, node, Evidence::Confirmed, "链")
            .unwrap();
        last = node;
        prev = node;
    }
    let reach = graph
        .reach(
            first,
            last,
            Limits {
                max_depth: 3,
                max_paths: 64,
            },
        )
        .unwrap();
    assert!(reach.truncated);
    assert_eq!(reach.verdict, Evidence::Unverified);
    assert_eq!(reach.reason, "truncated");
    // 条数预算同理。
    let reach = graph
        .reach(
            first,
            last,
            Limits {
                max_depth: 64,
                max_paths: 0,
            },
        )
        .unwrap();
    assert!(reach.truncated);
    assert_eq!(reach.verdict, Evidence::Unverified);
}

#[test]
fn fold_table_matches_k3_and_and_merge_matches_k3_or() {
    // 折叠表 3×3 穷举：路径折叠 ≡ K3 AND，跨路径合并 ≡ K3 OR。
    for a in [
        Evidence::RuledOut,
        Evidence::Unverified,
        Evidence::Confirmed,
    ] {
        for b in [
            Evidence::RuledOut,
            Evidence::Unverified,
            Evidence::Confirmed,
        ] {
            let expected_and = match (a.to_ternary(), b.to_ternary()) {
                (Ternary::False, _) | (_, Ternary::False) => Ternary::False,
                (Ternary::Unknown, _) | (_, Ternary::Unknown) => Ternary::Unknown,
                (Ternary::True, Ternary::True) => Ternary::True,
            };
            assert_eq!(a.fold(b).to_ternary(), expected_and, "{a:?} AND {b:?}");
            let expected_or = match (a.to_ternary(), b.to_ternary()) {
                (Ternary::True, _) | (_, Ternary::True) => Ternary::True,
                (Ternary::Unknown, _) | (_, Ternary::Unknown) => Ternary::Unknown,
                (Ternary::False, Ternary::False) => Ternary::False,
            };
            assert_eq!(a.merge(b).to_ternary(), expected_or, "{a:?} OR {b:?}");
        }
    }
}

#[test]
fn registration_discipline_is_enforced() {
    let mut graph = AttackGraph::new();
    let a = graph.add_node("a", Evidence::Confirmed, "");
    // 出处为空不许登记。
    assert_eq!(
        graph.add_edge(a, a, Evidence::Confirmed, ""),
        Err(AttackError::EmptySource)
    );
    // 未知端点不许登记。
    assert_eq!(
        graph.add_edge(a, 99, Evidence::Confirmed, "x"),
        Err(AttackError::UnknownNode(99))
    );
    // 同 label 复用同一节点（幂等）。
    let again = graph.add_node("a", Evidence::Unverified, "第二次登记");
    assert_eq!(a, again);
    assert_eq!(graph.len(), 1);
    // 同一对端点重复登记是 LWW：后写覆盖，并报告被覆盖的旧证据。
    let b = graph.add_node("b", Evidence::Confirmed, "");
    assert_eq!(
        graph.add_edge(a, b, Evidence::Unverified, "第一版"),
        Ok(None)
    );
    assert_eq!(
        graph.add_edge(a, b, Evidence::Confirmed, "复核后"),
        Ok(Some(Evidence::Unverified))
    );
    assert_eq!(graph.edge(a, b).unwrap().evidence, Evidence::Confirmed);
}

#[test]
fn jsonl_roundtrip_preserves_graph() {
    let mut graph = AttackGraph::new();
    let entry = graph.add_node("entry", Evidence::Confirmed, "立足点");
    let goal = graph.add_node("goal", Evidence::Unverified, "目标");
    graph
        .add_edge(entry, goal, Evidence::Unverified, "待复现")
        .unwrap();
    let text = graph.to_jsonl();
    let back = AttackGraph::from_jsonl(&text).unwrap();
    assert_eq!(graph, back);
    let reach = back.reach(entry, goal, Limits::default()).unwrap();
    assert_eq!(reach.verdict, Evidence::Unverified);
}

#[test]
fn jsonl_rejects_bad_rows_instead_of_dropping_them() {
    // 证据字符不认识。
    let bad_char =
        "{\"kind\":\"node\",\"id\":0,\"label\":\"a\",\"evidence\":\"Q\",\"note\":\"\"}\n";
    assert!(matches!(
        AttackGraph::from_jsonl(bad_char),
        Err(AttackError::Json(_))
    ));
    // 边端点没先登记——不静默丢行。
    let dangling = "{\"kind\":\"node\",\"id\":0,\"label\":\"a\",\"evidence\":\"T\",\"note\":\"\"}\n\
                    {\"kind\":\"edge\",\"from\":0,\"to\":1,\"evidence\":\"T\",\"source\":\"x\"}\n";
    assert!(matches!(
        AttackGraph::from_jsonl(dangling),
        Err(AttackError::Json(_))
    ));
    // 出处为空。
    let empty_source = "{\"kind\":\"node\",\"id\":0,\"label\":\"a\",\"evidence\":\"T\",\"note\":\"\"}\n\
                        {\"kind\":\"node\",\"id\":1,\"label\":\"b\",\"evidence\":\"T\",\"note\":\"\"}\n\
                        {\"kind\":\"edge\",\"from\":0,\"to\":1,\"evidence\":\"T\",\"source\":\"\"}\n";
    assert_eq!(
        AttackGraph::from_jsonl(empty_source),
        Err(AttackError::EmptySource)
    );
}

#[test]
fn unknown_nodes_are_reported_not_guessed() {
    let mut graph = AttackGraph::new();
    let a = graph.add_node("a", Evidence::Confirmed, "");
    assert_eq!(
        graph.reach(a, 42, Limits::default()),
        Err(AttackError::UnknownNode(42))
    );
    assert_eq!(
        graph.reach(42, a, Limits::default()),
        Err(AttackError::UnknownNode(42))
    );
}

#[test]
fn diamond_reports_every_route_with_its_own_fold() {
    // 菱形：T 路线 + X 路线 ⇒ K3 OR 下一条已证实路线就够，总判决 T；
    // 但两条明细各自带折叠与见证，X 路线的待验证环节不丢失。
    let mut graph = AttackGraph::new();
    let a = graph.add_node("a", Evidence::Confirmed, "");
    let left = graph.add_node("left", Evidence::Confirmed, "");
    let right = graph.add_node("right", Evidence::Confirmed, "");
    let goal = graph.add_node("goal", Evidence::Confirmed, "");
    graph
        .add_edge(a, left, Evidence::Confirmed, "左路")
        .unwrap();
    graph
        .add_edge(left, goal, Evidence::Confirmed, "左路")
        .unwrap();
    graph
        .add_edge(a, right, Evidence::Confirmed, "右路")
        .unwrap();
    graph
        .add_edge(right, goal, Evidence::Unverified, "右路待复现")
        .unwrap();
    let reach = graph.reach(a, goal, Limits::default()).unwrap();
    assert_eq!(reach.verdict, Evidence::Confirmed);
    assert_eq!(reach.reason, "proven-path");
    assert_eq!(reach.paths.len(), 2);
    let right_path = reach
        .paths
        .iter()
        .find(|path| path.nodes.contains(&right))
        .unwrap();
    assert_eq!(right_path.fold, Evidence::Unverified);
    assert_eq!(
        right_path.open,
        vec![OpenStep {
            node: right,
            edge_to: Some(goal),
        }]
    );
    // 把左路排除 ⇒ 只剩 X 路线，总判决退为 X。
    graph
        .add_edge(a, left, Evidence::RuledOut, "左路已封堵")
        .unwrap();
    let reach = graph.reach(a, goal, Limits::default()).unwrap();
    assert_eq!(reach.verdict, Evidence::Unverified);
    assert_eq!(reach.reason, "unverified-only");
}

#[test]
fn reach_struct_serializes_through_json() {
    // Reach/PathDetail/OpenStep 都要能进 JSON（供 CLI 与台账消费）。
    let reach = Reach {
        verdict: Evidence::Unverified,
        reason: "unverified-only".to_string(),
        truncated: false,
        paths: vec![PathDetail {
            nodes: vec![1, 2],
            fold: Evidence::Unverified,
            open: vec![OpenStep {
                node: 1,
                edge_to: Some(2),
            }],
        }],
    };
    let text = serde_json::to_string(&reach).unwrap();
    let back: Reach = serde_json::from_str(&text).unwrap();
    assert_eq!(back, reach);
}

#[test]
fn cli_attack_reports_verdict_and_witness_end_to_end() {
    let ledger = "{\"kind\":\"node\",\"id\":0,\"label\":\"entry\",\"evidence\":\"T\",\"note\":\"\"}\n\
                  {\"kind\":\"node\",\"id\":1,\"label\":\"dmz\",\"evidence\":\"T\",\"note\":\"\"}\n\
                  {\"kind\":\"node\",\"id\":2,\"label\":\"core\",\"evidence\":\"T\",\"note\":\"\"}\n\
                  {\"kind\":\"edge\",\"from\":0,\"to\":1,\"evidence\":\"T\",\"source\":\"已复现\"}\n\
                  {\"kind\":\"edge\",\"from\":1,\"to\":2,\"evidence\":\"X\",\"source\":\"待复现\"}\n";
    let path =
        std::env::temp_dir().join(format!("trit-netsec-attack-{}.jsonl", std::process::id()));
    std::fs::write(&path, ledger).expect("写临时台账");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_trit-net"))
        .arg("attack")
        .arg(&path)
        .arg("--src")
        .arg("entry")
        .arg("--dst")
        .arg("core")
        .output()
        .expect("运行 trit-net attack");
    let _ = std::fs::remove_file(&path);
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("输出是合法 JSON");
    assert_eq!(value["verdict"], "X");
    assert_eq!(value["reason"], "unverified-only");
    assert_eq!(value["paths"][0]["open"][0], "dmz -> core");
    // 未知节点如实报错退出。
    let path =
        std::env::temp_dir().join(format!("trit-netsec-attack-{}.jsonl", std::process::id()));
    std::fs::write(&path, ledger).expect("重写临时台账");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_trit-net"))
        .arg("attack")
        .arg(&path)
        .arg("--src")
        .arg("entry")
        .arg("--dst")
        .arg("不存在的节点")
        .output()
        .expect("运行 trit-net attack");
    let _ = std::fs::remove_file(&path);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("不在台账里"));
}
