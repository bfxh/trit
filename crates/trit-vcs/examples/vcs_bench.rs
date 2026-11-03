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

fn main() {
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
