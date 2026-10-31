//! 端到端场景演示：DeltaDB × 红蓝对抗 × 三进制合并门禁 × Delta 日志持久化。
//!
//! 全部变更通过 `Journal` 写入追加式日志，终局重新打开重放并断言状态一致。
//! 运行：`cargo run --example scenario`

use trit_immune::{
    scan, AttackVector, ExploitEvidence, Journal, JournalError, NodeId, NodeKind, Sandbox, Trit,
};

fn banner(title: &str) {
    println!("\n══════ {title} ══════");
}

/// journal 写入的统一落锤：失败即 panic（演示程序）。
fn jx<T>(r: Result<T, JournalError>) -> T {
    r.expect("journal 写入失败")
}

fn main() {
    let journal_path = std::env::temp_dir().join("trit-immune-scenario.journal");
    let _ = std::fs::remove_file(&journal_path);
    let mut j = Journal::create(&journal_path, "main").expect("创建 journal");
    let main = NodeId(0);
    let sandbox = Sandbox::new();

    // ── 第一幕 ──────────────────────────────────────────────────────────
    banner("第一幕：SQL 注入 —— 打穿 · 负数回链 · 传播波及 · 补丁 · 重放");
    let login = jx(j.spawn(main, NodeKind::Delta, "登录模块（蓝队 dev-01）"));
    let sqli = jx(j.spawn(login, NodeKind::AttackVector, "SQL 注入重放（红队 red-01）"));
    // 支付模块复用登录的会话解析 —— 一条跨树依赖边（传播通道）
    let pay = jx(j.spawn(main, NodeKind::Delta, "支付模块（蓝队 dev-04）"));
    jx(j.record(pay, Trit::Hardened, None, "自身回归通过"));
    jx(j.cross_link(
        login,
        pay,
        Trit::Vulnerable,
        "支付复用登录的会话解析",
    ));

    jx(j.record(
        sqli,
        Trit::Vulnerable,
        Some(ExploitEvidence {
            vector: AttackVector::SqlInjection,
            payload: "' OR '1'='1' --".into(),
            target: login,
            round: 3,
        }),
        "第 3 轮打穿登录接口",
    ));
    println!("  [红队] 打穿：payload = {:?} → -1（攻击成功）", "' OR '1'='1' --");
    println!("         负数消息回链：证据.target = #{:02}（写代码的那个节点）", login.0);
    jx(j.record(login, Trit::Vulnerable, None, "该攻击面已证伪（红队确认）"));
    println!("  [门禁] merge_gate(登录) = {}", j.dag().merge_gate(login));
    println!(
        "  [传播] 登录 -1 沿依赖边波及支付 → merge_gate(支付) = {}",
        j.dag().merge_gate(pay)
    );

    let patch = jx(j.spawn(login, NodeKind::Patch, "参数化查询补丁（蓝队 dev-01）"));
    jx(j.record(patch, Trit::Hardened, None, "回归测试通过"));
    jx(j.record(sqli, Trit::Hardened, None, "补丁后重放失败：驱动拒绝拼接语句"));
    jx(j.record(login, Trit::Hardened, None, "补丁合入并复验"));
    println!("  [蓝队] 补丁 #{:02} 落地；[红队] 重放 → +1", patch.0);
    println!("  [门禁] merge_gate(登录) = {}", j.dag().merge_gate(login));
    println!(
        "  [传播] 补丁后依赖边恢复 → merge_gate(支付) = {}",
        j.dag().merge_gate(pay)
    );

    // ── 第二幕 ──────────────────────────────────────────────────────────
    banner("第二幕：0 态 —— 冻结 · 沙盒模糊测试 · 升级");
    let comment = jx(j.spawn(main, NodeKind::Delta, "评论区组件（蓝队 dev-02）"));
    let xss = jx(j.spawn(comment, NodeKind::AttackVector, "XSS 探查（红队 red-02）"));
    let priv_esc = jx(j.spawn(comment, NodeKind::AttackVector, "越权探查（红队排队中）"));

    jx(j.record(xss, Trit::Unknown, None, "静态探查发现可疑反射点，证据不足"));
    println!("  [红队] 发现可疑反射面 → 0（冻结该分支，拉起沙盒）");
    println!("  [门禁] merge_gate(评论) = {}", j.dag().merge_gate(comment));

    // 挑一个会被沙盒打穿的疑点措辞（演示用；沙盒结果可离线复算）
    let candidates = ["反射输出未转义", "富文本过滤缺失", "DOM 注入面", "URL 参数回显"];
    let mut hit = None;
    for s in candidates {
        let r = sandbox.fuzz("评论区组件", s, AttackVector::Xss, 512);
        if r.verdict == Trit::Vulnerable {
            hit = Some((s, r));
            break;
        }
    }
    match hit {
        Some((s, r)) => {
            let payload = r.payload.clone().unwrap_or_default();
            jx(j.record(
                xss,
                Trit::Vulnerable,
                Some(ExploitEvidence {
                    vector: AttackVector::Xss,
                    payload: payload.clone(),
                    target: comment,
                    round: r.rounds,
                }),
                &format!("沙盒 {} 轮打穿（疑点：{s}）", r.rounds),
            ));
            println!("  [沙盒 DO-77] 第 {} 轮：{:?} 触发 → xss = -1", r.rounds, payload);
            println!("  [门禁] merge_gate(评论) = {}", j.dag().merge_gate(comment));
            jx(j.human_review(comment, Trit::Vulnerable, "人工确认：升级 P0，转修复"));
            let fix = jx(j.spawn(comment, NodeKind::Patch, "输出转义 + CSP（蓝队 dev-02）"));
            jx(j.record(fix, Trit::Hardened, None, "沙盒复测通过"));
            jx(j.record(xss, Trit::Hardened, None, "修复后重放失败"));
            jx(j.human_review(comment, Trit::Hardened, "人工复核放行"));
            println!("  [修复] #{:02} 落地，重放失败 → +1；人工复核放行", fix.0);
        }
        None => {
            let r = sandbox.fuzz("评论区组件", "反射输出未转义", AttackVector::Xss, 512);
            jx(j.record(
                xss,
                Trit::Hardened,
                None,
                &format!("沙盒 {} 轮未打穿 → 升级 +1", r.rounds),
            ));
            jx(j.record(comment, Trit::Hardened, None, "红队放行"));
            println!("  [沙盒] {} 轮未打穿 → xss = +1", r.rounds);
        }
    }
    println!(
        "  [门禁] merge_gate(评论) = {}（越权探查 #{:02} 仍在 0 态排队）",
        j.dag().merge_gate(comment),
        priv_esc.0
    );

    // ── 第三幕 ──────────────────────────────────────────────────────────
    banner("第三幕：Prompt 注入 —— 消息节点污染与因果链隔离");
    let docs = jx(j.spawn(main, NodeKind::Delta, "文档摘要服务（蓝队 dev-03）"));
    jx(j.record(docs, Trit::Hardened, None, "自身代码复测通过"));
    let page = jx(j.spawn(docs, NodeKind::Message, "抓取外部网页 content-42"));
    let gen = jx(j.spawn(page, NodeKind::Delta, "据该网页生成的过滤规则"));
    jx(j.record(gen, Trit::Hardened, None, "生成代码回归通过"));

    jx(j.quarantine(page, "上下文含『忽略以前指令』注入模式"));
    println!(
        "  [红队] 消息 #{:02} 被污染 → 隔离沿因果链传播（含 #{:02}）",
        page.0, gen.0
    );
    println!("  [门禁] merge_gate(生成规则) = {}", j.dag().merge_gate(gen));

    jx(j.human_review(page, Trit::Hardened, "人工复核：网页内容无实际注入"));
    jx(j.human_review(gen, Trit::Hardened, "人工复核：生成规则无后门"));
    println!("  [人类] 0 态落锤：解除隔离 → merge_gate = {}", j.dag().merge_gate(gen));

    // ── 第四幕 ──────────────────────────────────────────────────────────
    banner("第四幕：静态扫描 —— 规则引擎作为三值裁决源");
    let cfg = jx(j.spawn(main, NodeKind::Delta, "配置加载模块（蓝队 dev-05）"));
    jx(j.record(cfg, Trit::Hardened, None, "自身回归通过"));
    let render = jx(j.spawn(main, NodeKind::Delta, "评论渲染增强（蓝队 dev-06）"));
    jx(j.record(render, Trit::Hardened, None, "自身回归通过"));

    let snippets = [
        (cfg, "db.password = \"hunter2\";"),
        (render, "el.innerHTML = userInput;"),
    ];
    let report = scan::run_scan(&snippets, scan::RULES, &[]);
    for f in &report.findings {
        println!("  [扫描] #{:02} 命中 {} → {}", f.node.0, f.rule, f.calibrated);
    }
    for d in report.to_deltas("TRI") {
        jx(j.append_delta(&d));
    }
    println!(
        "  [门禁] merge_gate(配置加载) = {}（规则即漏洞本体，直接证伪）",
        j.dag().merge_gate(cfg)
    );
    println!(
        "  [门禁] merge_gate(渲染增强) = {}（启发式怀疑，冻结交红队）",
        j.dag().merge_gate(render)
    );
    println!("  [边界] 本轮扫描产出 0 条 +1：清白的扫描只是『没发现』，不等于『安全』");

    // ── 终局：落盘 → 重放 → 断言一致 ────────────────────────────────────
    banner("终局：免疫网络全景");
    println!("{}", j.dag().render());
    let queue: Vec<String> = j
        .dag()
        .suspects()
        .iter()
        .map(|id| format!("#{:02} {}", id.0, j.dag().node(*id).title))
        .collect();
    println!("待办（0 态队列）：[{}]", queue.join(", "));

    let snap_path = j.snapshot().expect("写快照");
    let snap_size = std::fs::metadata(&snap_path).map(|m| m.len()).unwrap_or(0);
    let saved = j.into_dag().expect("journal flush");
    let raw = std::fs::read_to_string(&journal_path).expect("读 journal");
    println!(
        "\n[持久化] journal {} 字节、{} 条 Delta；快照 {} 字节；前 4 行：",
        raw.len(),
        raw.lines().count() - 1,
        snap_size
    );
    for line in raw.lines().take(4) {
        println!("    {line}");
    }

    let mut r1 = Journal::open(&journal_path).expect("重放 journal");
    assert_eq!(r1.dag(), &saved, "重放状态必须与实时状态一致");
    println!(
        "[快照] 第 1 次打开：走快照={}，尾部重放 {} 条；恢复 {} 个节点 ✓",
        r1.recovered_from_snapshot(),
        r1.tail_replayed(),
        r1.dag().nodes().len()
    );

    // 快照之后继续追加 → 下次打开只重放尾部增量
    jx(r1.record(pay, Trit::Hardened, None, "依赖边复测通过"));
    r1.into_dag().expect("flush");
    let r2 = Journal::open(&journal_path).expect("第二次打开");
    println!(
        "[快照] 第 2 次打开：走快照={}，尾部重放 {} 条 ✓",
        r2.recovered_from_snapshot(),
        r2.tail_replayed()
    );
    println!(
        "[时钟] 写者 #{}，HLC 最后戳 {}（每条 Delta 均盖戳，非递增日志直接拒绝）",
        r2.clock().writer(),
        r2.clock().last()
    );

    println!("\n门禁总览（重放后）：");
    for (name, tip) in [
        ("登录", login),
        ("支付", pay),
        ("配置", cfg),
        ("渲染", render),
        ("评论", comment),
        ("文档", docs),
    ] {
        println!("  merge_gate({name}) = {}", r2.dag().merge_gate(tip));
    }
}
