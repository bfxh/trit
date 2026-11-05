//! 同一份语料的「Git vs trit-vcs」对照：存体积与写入耗时。
//!
//! 口径：先生成 N 个文件，再在**同一个文件中间插入一段**重复提交 20 次——
//! 这正是内容定义分块应该赢的形状（Git 的整文件 blob 每次都要新建整块）。
//! 每条读数都打印来源，便于复算。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use trit_vcs::gate::ClaimSpec;
use trit_vcs::repo::{CommitRequest, Repo};

fn corpus(files: usize, lines: usize) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    for f in 0..files {
        let mut body = String::new();
        for line in 0..lines {
            body.push_str(&format!(
                "fn item_{f}_{line}() -> u32 {{ {} }}\n",
                (f * 131 + line * 17) % 997
            ));
        }
        out.insert(format!("gen/file_{f:03}.rs"), body.into_bytes());
    }
    out
}

fn dir_bytes(root: &std::path::Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(meta) = entry.metadata() {
                total += meta.len();
            }
        }
    }
    total
}

fn inject(entry: &mut Vec<u8>) {
    let cut = entry.len() / 2;
    let mut injected = b"// inserted by vcs_bench\n".to_vec();
    injected.extend(entry.split_off(cut));
    entry.extend_from_slice(&injected);
}

/// 场景 B：60 个文件共享同一个 100 KiB 大块（只有头部不同）。
/// 这是块级去重该赢的形状：git 只对"整文件相同"做 blob 去重，共享块要靠 delta 猜。
fn shared_block(files: usize, block: usize) -> BTreeMap<String, Vec<u8>> {
    let mut state = 0x1234_5678_9abc_def0u64;
    let mut common = Vec::with_capacity(block);
    for _ in 0..block {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        common.push(b'a' + (state % 26) as u8);
    }
    let mut out = BTreeMap::new();
    for f in 0..files {
        let mut body = format!(
            "// unique header {f}
"
        )
        .into_bytes();
        body.extend_from_slice(&common);
        out.insert(format!("shared/file_{f:03}.rs"), body);
    }
    out
}

fn bench_shared_block() {
    let files = 60usize;
    let block = 100 * 1024usize;
    let base = shared_block(files, block);
    let payload: u64 = base.values().map(|v| v.len() as u64).sum();
    let root: PathBuf =
        std::env::temp_dir().join(format!("trit-vcs-bench-shared-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let repo_root = root.join("trit");
    std::fs::create_dir_all(&repo_root).unwrap();
    let mut repo = Repo::init(&repo_root).unwrap();
    repo.stage(&base, 1_000).unwrap();
    repo.commit(CommitRequest {
        message: "shared".to_string(),
        session: "qoder:bench".to_string(),
        claims: vec![ClaimSpec {
            id: "c1".to_string(),
            owner: "qoder:bench".to_string(),
            paths: vec!["shared".to_string()],
            acquired_at: 1,
            expires_at: u64::MAX / 2,
        }],
        now_secs: 1_000,
        ..Default::default()
    })
    .unwrap();
    let vcs_bytes = dir_bytes(&repo_root.join(".tritvcs").join("objects"));

    let git_root = root.join("git");
    std::fs::create_dir_all(&git_root).unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(&git_root)
            .output()
            .expect("git 不可用");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "bench@example.com"]);
    git(&["config", "user.name", "bench"]);
    for (path, body) in &base {
        let target = git_root.join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, body).unwrap();
    }
    git(&["add", "-A"]);
    git(&["commit", "-qm", "shared"]);
    let git_bytes = dir_bytes(&git_root.join(".git"));

    println!(
        "场景 B（共享块）：{} 文件各带同一个 {} KiB 块，原始 {:.2} MiB",
        files,
        block / 1024,
        payload as f64 / 1_048_576.0
    );
    println!(
        "  trit-vcs {:.2} MiB ｜ git {:.2} MiB ｜ 体积比 {:.2}",
        vcs_bytes as f64 / 1_048_576.0,
        git_bytes as f64 / 1_048_576.0,
        vcs_bytes as f64 / git_bytes.max(1) as f64
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// 场景 C：大文件反复"整体重编号"（每行都变一点）。
/// 这是 delta 该赢的形状：CDC 会认为每个块都变了（内容确实都变了），
/// 但块内的前后缀高度相似——整块存下来就亏，增量才省。
fn bench_renumber() {
    let lines = 20_000usize;
    let rounds = 10usize;
    let make = |tag: usize| -> Vec<u8> {
        let mut body = String::new();
        for line in 0..lines {
            body.push_str(&format!(
                "let v_{line} = compute({line}, {tag}, {tag});
"
            ));
        }
        body.into_bytes()
    };
    let root: PathBuf =
        std::env::temp_dir().join(format!("trit-vcs-bench-renum-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let repo_root = root.join("trit");
    std::fs::create_dir_all(&repo_root).unwrap();
    let mut repo = Repo::init(&repo_root).unwrap();
    let claim = ClaimSpec {
        id: "c1".to_string(),
        owner: "qoder:bench".to_string(),
        paths: vec!["big.rs".to_string()],
        acquired_at: 1,
        expires_at: u64::MAX / 2,
    };
    for round in 0..rounds {
        let mut files = BTreeMap::new();
        files.insert("big.rs".to_string(), make(round));
        repo.stage(&files, 1_000 + round as u64).unwrap();
        repo.commit(CommitRequest {
            message: format!("renumber {round}"),
            session: "qoder:bench".to_string(),
            claims: vec![claim.clone()],
            now_secs: 1_000 + round as u64,
            ..Default::default()
        })
        .unwrap();
    }
    let vcs_bytes = dir_bytes(&repo_root.join(".tritvcs").join("objects"));

    let git_root = root.join("git");
    std::fs::create_dir_all(&git_root).unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(&git_root)
            .output()
            .expect("git 不可用");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "bench@example.com"]);
    git(&["config", "user.name", "bench"]);
    for round in 0..rounds {
        std::fs::write(git_root.join("big.rs"), make(round)).unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", &format!("renumber {round}")]);
    }
    let git_bytes = dir_bytes(&git_root.join(".git"));
    let raw = make(0).len() as u64;
    println!(
        "场景 C（整体重编号）：单文件 {:.2} MiB × {} 轮（每行都变一点）",
        raw as f64 / 1_048_576.0,
        rounds
    );
    println!(
        "  trit-vcs {:.2} MiB ｜ git {:.2} MiB ｜ 体积比 {:.2}",
        vcs_bytes as f64 / 1_048_576.0,
        git_bytes as f64 / 1_048_576.0,
        vcs_bytes as f64 / git_bytes.max(1) as f64
    );
    let _ = std::fs::remove_dir_all(&root);
}

fn floor_walk(root: &std::path::Path) -> (std::time::Duration, usize) {
    let start = Instant::now();
    let mut stack = vec![root.to_path_buf()];
    let mut files = 0usize;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == ".tritvcs" || name == ".git" {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                files += 1;
            }
        }
    }
    (start.elapsed(), files)
}

/// 场景 D：10k 文件的热 `status` 对照（stat 缓存 vs git 的 index 缓存），外加"纯 stat"地板。
fn status_bench() {
    use trit_vcs::worktree;

    let files = 10_000usize;
    let root = std::env::temp_dir().join(format!("trit-vcs-status-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let trit_root = root.join("trit");
    let git_root = root.join("git");

    let mut entries = BTreeMap::new();
    for i in 0..files {
        let body = format!("file {i:05}\n").repeat(8);
        entries.insert(
            format!("gen/d{:03}/f{i:05}.txt", i / 100),
            body.into_bytes(),
        );
    }

    std::fs::create_dir_all(&trit_root).unwrap();
    let mut repo = Repo::init(&trit_root).unwrap();
    for (path, bytes) in &entries {
        let target = trit_root.join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, bytes).unwrap();
    }
    repo.stage(&entries, 1_000).unwrap();
    repo.commit(CommitRequest {
        message: "status bench".to_string(),
        session: "qoder:bench".to_string(),
        claims: vec![ClaimSpec {
            id: "c1".to_string(),
            owner: "qoder:bench".to_string(),
            paths: vec!["gen".to_string()],
            acquired_at: 1,
            expires_at: u64::MAX / 2,
        }],
        now_secs: 1_000,
        ..Default::default()
    })
    .unwrap();

    let cold_start = Instant::now();
    let (cold_changes, cold_stats) = worktree::status_with_cache(&repo).unwrap();
    let cold = cold_start.elapsed();
    assert!(cold_changes.is_empty());

    let mut warm_best = std::time::Duration::MAX;
    let mut warm_stats = worktree::StatusStats::default();
    for _ in 0..3 {
        let start = Instant::now();
        let (changes, stats) = worktree::status_with_cache(&repo).unwrap();
        let elapsed = start.elapsed();
        assert!(changes.is_empty());
        warm_stats = stats;
        warm_best = warm_best.min(elapsed);
    }

    let (floor, floor_files) = floor_walk(&trit_root);

    std::fs::create_dir_all(&git_root).unwrap();
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(&git_root)
            .output()
            .expect("git 不可用");
        assert!(
            status.status.success(),
            "git {args:?} 失败：{}",
            String::from_utf8_lossy(&status.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "bench@example.com"]);
    git(&["config", "user.name", "bench"]);
    for (path, bytes) in &entries {
        let target = git_root.join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, bytes).unwrap();
    }
    git(&["add", "-A"]);
    git(&["commit", "-qm", "base"]);

    let mut git_best = std::time::Duration::MAX;
    for _ in 0..3 {
        let start = Instant::now();
        let output = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&git_root)
            .output()
            .expect("git 不可用");
        assert!(output.status.success());
        assert!(output.stdout.is_empty(), "git status 应干净");
        git_best = git_best.min(start.elapsed());
    }

    let cold_ms = cold.as_secs_f64() * 1000.0;
    let warm_ms = warm_best.as_secs_f64() * 1000.0;
    let git_ms = git_best.as_secs_f64() * 1000.0;
    let ratio = warm_ms / git_ms.max(0.001);
    println!("场景 D：{} 个文件（{} 目录），同机 3 次取最好", files, 100);
    println!(
        "  trit 冷：{cold_ms:.0} ms（读 {} / 缓存 {}）",
        cold_stats.hashed, cold_stats.cached
    );
    println!(
        "  trit 热：{warm_ms:.0} ms（读 {} / 缓存 {}）",
        warm_stats.hashed, warm_stats.cached
    );
    println!("  git  热：{git_ms:.0} ms");
    println!(
        "  地板（纯走目录 + metadata）：{:.0} ms（{} 文件）",
        floor.as_secs_f64() * 1000.0,
        floor_files
    );
    println!("  trit/git = {ratio:.1}×（目标 ≤2×；地板决定下限）");
    let _ = std::fs::remove_dir_all(&root);
}

/// 场景 E：结构化争用对象的成本（对象字节 vs 两份全文），以及伪冲突是否为 0。
fn merge_bench() {
    use trit_vcs::merge;

    let root = std::env::temp_dir().join(format!("trit-vcs-merge-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut repo = Repo::init(&root).unwrap();
    let commit = |repo: &mut Repo, body: &str, parent: Option<trit_vcs::Hash>, now: u64| {
        let files: BTreeMap<String, Vec<u8>> =
            [("f.txt".to_string(), body.as_bytes().to_vec())].into();
        repo.stage(&files, now).unwrap();
        repo.commit(CommitRequest {
            message: format!("c{now}"),
            session: "qoder:bench".to_string(),
            claims: vec![ClaimSpec {
                id: format!("c{now}"),
                owner: "qoder:bench".to_string(),
                paths: vec!["f.txt".to_string()],
                acquired_at: 1,
                expires_at: u64::MAX / 2,
            }],
            parent_override: parent,
            now_secs: now,
            ..Default::default()
        })
        .unwrap()
        .id
    };

    let base_text: String = (0..300).map(|i| format!("line {i}\n")).collect();
    let ours_text: String = (0..300)
        .map(|i| {
            if i == 150 {
                "ours changed\n".to_string()
            } else {
                format!("line {i}\n")
            }
        })
        .collect();
    let theirs_text: String = (0..300)
        .map(|i| {
            if i == 150 {
                "theirs changed\n".to_string()
            } else {
                format!("line {i}\n")
            }
        })
        .collect();
    let base = commit(&mut repo, &base_text, None, 1_000);
    let ours = commit(&mut repo, &ours_text, Some(base), 1_001);
    let theirs = commit(&mut repo, &theirs_text, Some(base), 1_002);
    let outcome = merge::merge(&repo, &ours, &theirs, None).unwrap();
    let conflict = &outcome.conflicts[0];
    println!(
        "冲突对象 {} B vs 两侧全文 {} B（{:.2}%，hunks={}）",
        conflict.object_bytes(),
        conflict.raw_bytes,
        conflict.object_bytes() as f64 / conflict.raw_bytes as f64 * 100.0,
        conflict.hunks.len()
    );

    // 伪冲突：一侧只加行尾空白，另一侧改真实行 ⇒ 冲突数必须为 0。
    let format_ours = commit(
        &mut repo,
        &base_text.replace("line 2\n", "line 2   \n"),
        Some(base),
        1_003,
    );
    let format_theirs = commit(
        &mut repo,
        &base_text.replace("line 250\n", "changed 250\n"),
        Some(base),
        1_004,
    );
    let outcome = merge::merge(&repo, &format_ours, &format_theirs, None).unwrap();
    println!("伪冲突（格式化）冲突数 = {}", outcome.conflicts.len());
    let _ = std::fs::remove_dir_all(&root);
}

/// 场景 F：中心化 + 按需物化——全量 publish 的字节，对上单目录 pull 的字节与用时。
fn central_bench() {
    use trit_vcs::central;

    let dirs = 20usize;
    let per = 10usize;
    let root = std::env::temp_dir().join(format!("trit-vcs-central-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);

    let mut entries: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for d in 0..dirs {
        for f in 0..per {
            entries.insert(
                format!("gen/d{d:02}/f{f:02}.txt"),
                format!("dir {d} file {f}\n").repeat(40).into_bytes(),
            );
        }
    }

    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    let mut repo = Repo::init(&src).unwrap();
    repo.stage(&entries, 1_000).unwrap();
    repo.commit(CommitRequest {
        message: "central bench".to_string(),
        session: "qoder:bench".to_string(),
        claims: vec![ClaimSpec {
            id: "c1".to_string(),
            owner: "qoder:bench".to_string(),
            paths: vec!["gen".to_string()],
            acquired_at: 1,
            expires_at: u64::MAX / 2,
        }],
        now_secs: 1_000,
        ..Default::default()
    })
    .unwrap();

    let hub_dir = root.join("central");
    std::fs::create_dir_all(&hub_dir).unwrap();
    let mut hub = Repo::init(&hub_dir).unwrap();
    let start = Instant::now();
    let published = central::publish(&repo, &mut hub, "main", None, false).unwrap();
    let publish_ms = start.elapsed().as_secs_f64() * 1000.0;

    let one_dir = root.join("client-one");
    std::fs::create_dir_all(&one_dir).unwrap();
    let mut client = Repo::init(&one_dir).unwrap();
    let start = Instant::now();
    let first = central::pull(&hub, &mut client, "main", Some("gen/d00"), false).unwrap();
    let first_ms = start.elapsed().as_secs_f64() * 1000.0;
    let again = central::pull(&hub, &mut client, "main", Some("gen/d00"), false).unwrap();

    let all_dir = root.join("client-all");
    std::fs::create_dir_all(&all_dir).unwrap();
    let mut client_all = Repo::init(&all_dir).unwrap();
    let start = Instant::now();
    let whole = central::pull(&hub, &mut client_all, "main", None, false).unwrap();
    let whole_ms = start.elapsed().as_secs_f64() * 1000.0;

    let ratio = first.transfer.sent_bytes as f64 / published.transfer.total_bytes as f64 * 100.0;
    println!(
        "F 中心化（{} 目录 × {} 文件 = {} 个）",
        dirs,
        per,
        dirs * per
    );
    println!(
        "  publish 全量：{} B / {} 对象（{:.1} ms）",
        published.transfer.total_bytes, published.transfer.objects, publish_ms
    );
    println!(
        "  单目录 pull：{} B / {} 对象 ⇒ {:.2}% 全量；物化 {} 个文件、范围外跳过 {}（{:.1} ms）",
        first.transfer.sent_bytes,
        first.transfer.objects,
        ratio,
        first.written,
        first.skipped,
        first_ms
    );
    println!(
        "  重复 pull：实传 {} B / {} 对象（判据 0；跳过 {}）",
        again.transfer.sent_bytes, again.transfer.sent_objects, again.transfer.skipped
    );
    println!(
        "  全量 pull：{} B；物化 {} 个文件（{:.1} ms）",
        whole.transfer.sent_bytes, whole.written, whole_ms
    );
    let _ = std::fs::remove_dir_all(&root);
}

fn main() {
    if std::env::args().any(|arg| arg == "--status") {
        status_bench();
        return;
    }
    if std::env::args().any(|arg| arg == "--central") {
        central_bench();
        return;
    }
    if std::env::args().any(|arg| arg == "--merge") {
        merge_bench();
        return;
    }
    let files = 60usize;
    let lines = 400usize;
    let rounds = 20usize;
    let base = corpus(files, lines);
    let payload_bytes: u64 = base.values().map(|v| v.len() as u64).sum();

    let root: PathBuf = std::env::temp_dir().join(format!("trit-vcs-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    // —— trit-vcs：每轮在第一个文件中间插 64 字节 ——
    let repo_root = root.join("trit");
    std::fs::create_dir_all(&repo_root).unwrap();
    let mut repo = Repo::init(&repo_root).unwrap();
    let mut current = base.clone();
    let claim = ClaimSpec {
        id: "c1".to_string(),
        owner: "qoder:bench".to_string(),
        paths: vec!["gen".to_string()],
        acquired_at: 1,
        expires_at: u64::MAX / 2,
    };
    let start = Instant::now();
    for round in 0..rounds {
        inject(current.get_mut("gen/file_000.rs").unwrap());
        repo.stage(&current, 1_000 + round as u64).unwrap();
        repo.commit(CommitRequest {
            message: format!("round {round}"),
            session: "qoder:bench".to_string(),
            claims: vec![claim.clone()],
            now_secs: 1_000 + round as u64,
            ..Default::default()
        })
        .unwrap();
    }
    let vcs_time = start.elapsed();
    let vcs_bytes = dir_bytes(&repo_root.join(".tritvcs").join("objects"));

    // —— Git：同样的 20 轮 ——
    let git_root = root.join("git");
    std::fs::create_dir_all(&git_root).unwrap();
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(&git_root)
            .output()
            .expect("git 不可用");
        assert!(
            status.status.success(),
            "git {args:?} 失败：{}",
            String::from_utf8_lossy(&status.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "bench@example.com"]);
    git(&["config", "user.name", "bench"]);
    let mut git_current = base.clone();
    let start = Instant::now();
    for round in 0..rounds {
        for (path, body) in &git_current {
            let target = git_root.join(path);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(&target, body).unwrap();
        }
        git(&["add", "-A"]);
        git(&["commit", "-qm", &format!("round {round}")]);
        inject(git_current.get_mut("gen/file_000.rs").unwrap());
    }
    let git_time = start.elapsed();
    let git_bytes = dir_bytes(&git_root.join(".git"));

    println!(
        "语料：{} 个文件 × {} 行，原始 {:.2} MiB，{} 轮「中间插 64 字节」",
        files,
        lines,
        payload_bytes as f64 / 1_048_576.0,
        rounds
    );
    let size_of = |path: PathBuf| std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let pack = size_of(
        repo_root
            .join(".tritvcs")
            .join("objects")
            .join("pack-000001.pack"),
    );
    let index = size_of(
        repo_root
            .join(".tritvcs")
            .join("objects")
            .join("index.jsonl"),
    );
    println!(
        "trit-vcs：对象库 {:.2} MiB（pack {:.2} / 索引 {:.2}），{:.0} ms/轮",
        vcs_bytes as f64 / 1_048_576.0,
        pack as f64 / 1_048_576.0,
        index as f64 / 1_048_576.0,
        vcs_time.as_secs_f64() * 1000.0 / rounds as f64
    );
    println!(
        "git      ：.git     {:.2} MiB，{:.0} ms/轮",
        git_bytes as f64 / 1_048_576.0,
        git_time.as_secs_f64() * 1000.0 / rounds as f64
    );
    println!(
        "体积比 trit/git = {:.2}（越小越省；这一栏只测存储，不测 checkout/merge）",
        vcs_bytes as f64 / git_bytes.max(1) as f64
    );
    let _ = std::fs::remove_dir_all(&root);
    bench_shared_block();
    bench_renumber();
}
