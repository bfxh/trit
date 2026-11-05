use super::*;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("trit-vcs-repo-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn claim(owner: &str, paths: &[&str], expires_at: u64) -> ClaimSpec {
    ClaimSpec {
        id: format!("c-{owner}"),
        owner: owner.to_string(),
        paths: paths.iter().map(|p| p.to_string()).collect(),
        acquired_at: 1,
        expires_at,
    }
}

fn files(pairs: &[(&str, &str)]) -> BTreeMap<String, Vec<u8>> {
    pairs
        .iter()
        .map(|(path, body)| (path.to_string(), body.as_bytes().to_vec()))
        .collect()
}

fn run_witness(tree: Hash, base: Option<Hash>, exit: i32) -> Vec<u8> {
    crate::witness::Witness::from_run(
        tree,
        base,
        vec!["cargo".to_string(), "test".to_string()],
        crate::witness::RunOutput::new(exit, b"ok".to_vec(), Vec::new(), 1_000, 5),
    )
    .encode()
}

#[test]
fn stage_commit_and_log() {
    let root = temp("basic");
    let mut repo = Repo::init(&root).unwrap();
    repo.stage(&files(&[("a.txt", "alpha")]), 1_000).unwrap();
    let outcome = repo
        .commit(CommitRequest {
            message: "first".to_string(),
            session: "qoder:s1".to_string(),
            claims: vec![claim("qoder:s1", &["a.txt"], 2_000)],
            evidence: vec!["PR #1".to_string()],
            tags: vec!["demo".to_string()],
            allow_unclaimed: false,
            witness: None,
            intent: None,
            parent_override: None,
            now_secs: 1_000,
        })
        .unwrap();
    assert_eq!(outcome.verdict, Verdict::Allowed);
    let log = repo.log().unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].1.message, "first");
    assert_eq!(log[0].1.evidence, vec!["PR #1".to_string()]);
    let report = repo.verify().unwrap();
    assert_eq!(report.commits, 1);
    assert!(report.objects > 0);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn unclaimed_commit_is_rejected_by_default() {
    let root = temp("unclaimed");
    let mut repo = Repo::init(&root).unwrap();
    repo.stage(&files(&[("a.txt", "alpha")]), 1_000).unwrap();
    let err = repo
        .commit(CommitRequest {
            message: "sneaky".to_string(),
            session: "qoder:s1".to_string(),
            now_secs: 1_000,
            ..Default::default()
        })
        .unwrap_err();
    assert!(matches!(err, RepoError::Unclaimed { .. }), "{err:?}");
    assert!(repo.head().unwrap().is_none(), "被拒的提交不许推进 HEAD");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn unclaimed_commit_can_be_forced_and_is_recorded() {
    let root = temp("force");
    let mut repo = Repo::init(&root).unwrap();
    repo.stage(&files(&[("a.txt", "alpha")]), 1_000).unwrap();
    repo.commit(CommitRequest {
        message: "forced".to_string(),
        session: "qoder:s1".to_string(),
        allow_unclaimed: true,
        now_secs: 1_000,
        ..Default::default()
    })
    .unwrap();
    let last = repo.journal().events().last().unwrap();
    assert_eq!(last.kind, "commit");
    assert_eq!(last.payload["verdict"], "unclaimed", "放行也要留痕");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn conflicting_claim_blocks() {
    let root = temp("conflict");
    let mut repo = Repo::init(&root).unwrap();
    repo.stage(&files(&[("a.txt", "alpha")]), 1_000).unwrap();
    let err = repo
        .commit(CommitRequest {
            message: "clash".to_string(),
            session: "qoder:s1".to_string(),
            claims: vec![claim("zcode:zoder", &["a.txt"], 2_000)],
            now_secs: 1_000,
            ..Default::default()
        })
        .unwrap_err();
    match err {
        RepoError::Conflict { others, .. } => {
            assert_eq!(others, vec!["zcode:zoder".to_string()])
        }
        other => panic!("期望冲突，得到 {other:?}"),
    }
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn second_commit_links_to_first() {
    let root = temp("chain");
    let mut repo = Repo::init(&root).unwrap();
    let claims = vec![claim("qoder:s1", &["a.txt"], 9_999)];
    repo.stage(&files(&[("a.txt", "one")]), 1_000).unwrap();
    let first = repo
        .commit(CommitRequest {
            message: "one".to_string(),
            session: "qoder:s1".to_string(),
            claims: claims.clone(),
            now_secs: 1_000,
            ..Default::default()
        })
        .unwrap();
    repo.stage(&files(&[("a.txt", "two")]), 2_000).unwrap();
    let second = repo
        .commit(CommitRequest {
            message: "two".to_string(),
            session: "qoder:s1".to_string(),
            claims,
            now_secs: 2_000,
            ..Default::default()
        })
        .unwrap();
    let log = repo.log().unwrap();
    assert_eq!(log.len(), 2);
    // log 是新→旧：log[0] 是第二条，它的 parent 必须是第一条
    assert_eq!(log[0].0, second.id);
    assert_eq!(log[0].1.parents, vec![first.id]);
    assert!(log[1].1.parents.is_empty(), "第一条没有父提交");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn journal_and_objects_are_verified_together() {
    let root = temp("verify");
    let mut repo = Repo::init(&root).unwrap();
    repo.stage(&files(&[("a.txt", "alpha")]), 1_000).unwrap();
    repo.commit(CommitRequest {
        message: "x".to_string(),
        session: "qoder:s1".to_string(),
        claims: vec![claim("qoder:s1", &["a.txt"], 9_999)],
        now_secs: 1_000,
        ..Default::default()
    })
    .unwrap();
    assert!(repo.verify().is_ok());
    // 篡改 pack 里的一个字节 ⇒ verify 必须红
    let pack = Repo::dir(&root).join("objects").join("pack-000001.pack");
    let mut bytes = fs::read(&pack).unwrap();
    let at = bytes.len() / 2;
    bytes[at] ^= 0xFF;
    fs::write(&pack, bytes).unwrap();
    assert!(repo.verify().is_err());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn commit_with_many_files_uses_claim_scopes() {
    let root = temp("many-paths");
    let mut repo = Repo::init(&root).unwrap();
    let mut files = BTreeMap::new();
    for index in 0..300 {
        files.insert(
            format!("gen/f{index:04}.txt"),
            format!("v{index}").into_bytes(),
        );
    }
    repo.stage(&files, 1_000).unwrap();
    let outcome = repo
        .commit(CommitRequest {
            message: "many".to_string(),
            session: "qoder:s1".to_string(),
            claims: vec![claim("qoder:s1", &["gen"], 9_999)],
            now_secs: 1_000,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(outcome.verdict, Verdict::Allowed);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn policy_can_require_a_witness() {
    let root = temp("witness-required");
    let mut repo = Repo::init(&root).unwrap();
    fs::write(
        Repo::dir(&root).join("policy.json"),
        br#"{"require_witness":true}"#,
    )
    .unwrap();
    repo.stage(&files(&[("a.txt", "alpha")]), 1_000).unwrap();
    let err = repo
        .commit(CommitRequest {
            message: "no witness".to_string(),
            session: "qoder:s1".to_string(),
            claims: vec![claim("qoder:s1", &["a.txt"], 9_999)],
            now_secs: 1_000,
            ..Default::default()
        })
        .unwrap_err();
    assert!(matches!(err, RepoError::WitnessRequired), "{err:?}");
    assert!(repo.head().unwrap().is_none(), "没有见证不许推进 HEAD");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn policy_can_require_an_intent() {
    let root = temp("intent-required");
    let mut repo = Repo::init(&root).unwrap();
    fs::write(
        Repo::dir(&root).join("policy.json"),
        br#"{"require_intent":true}"#,
    )
    .unwrap();
    repo.stage(&files(&[("a.txt", "alpha")]), 1_000).unwrap();
    let err = repo
        .commit(CommitRequest {
            message: "no intent".to_string(),
            session: "qoder:s1".to_string(),
            claims: vec![claim("qoder:s1", &["a.txt"], 9_999)],
            now_secs: 1_000,
            ..Default::default()
        })
        .unwrap_err();
    assert!(matches!(err, RepoError::IntentRequired), "{err:?}");
    assert!(repo.head().unwrap().is_none());

    let outcome = repo
        .commit(CommitRequest {
            message: "with intent".to_string(),
            session: "qoder:s1".to_string(),
            claims: vec![claim("qoder:s1", &["a.txt"], 9_999)],
            intent: Some(Intent::new("写清楚为什么改", vec!["门禁全绿".to_string()])),
            now_secs: 1_001,
            ..Default::default()
        })
        .unwrap();
    let commit = repo.commit_at(&outcome.id).unwrap();
    assert_eq!(commit.intent.unwrap().goal, "写清楚为什么改".to_string());
    assert!(repo.head().unwrap().is_some());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn intent_needs_a_goal_and_accept() {
    let root = temp("intent-invalid");
    let mut repo = Repo::init(&root).unwrap();
    repo.stage(&files(&[("a.txt", "alpha")]), 1_000).unwrap();
    for intent in [
        Intent::new("  ", vec!["x".to_string()]),
        Intent::new("goal", Vec::new()),
        Intent::new("goal", vec!["   ".to_string()]),
    ] {
        let err = repo
            .commit(CommitRequest {
                message: "bad".to_string(),
                session: "qoder:s1".to_string(),
                claims: vec![claim("qoder:s1", &["a.txt"], 9_999)],
                intent: Some(intent),
                now_secs: 1_000,
                ..Default::default()
            })
            .unwrap_err();
        assert!(matches!(err, RepoError::IntentInvalid(_)), "{err:?}");
    }
    assert!(repo.head().unwrap().is_none());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn intent_base_must_match_parent_tree() {
    let root = temp("intent-base");
    let mut repo = Repo::init(&root).unwrap();
    let claims = vec![claim("qoder:s1", &["a.txt"], 9_999)];
    repo.stage(&files(&[("a.txt", "v1")]), 1_000).unwrap();
    let first = repo
        .commit(CommitRequest {
            message: "v1".to_string(),
            session: "qoder:s1".to_string(),
            claims: claims.clone(),
            now_secs: 1_000,
            ..Default::default()
        })
        .unwrap();
    let first_tree = repo.commit_at(&first.id).unwrap().tree;

    repo.stage(&files(&[("a.txt", "v2")]), 1_001).unwrap();
    let err = repo
        .commit(CommitRequest {
            message: "stale base".to_string(),
            session: "qoder:s1".to_string(),
            claims: claims.clone(),
            intent: Some(
                Intent::new("改 v2", vec!["v2 在位".to_string()])
                    .with_base(Hash::of(b"wrong-base")),
            ),
            now_secs: 1_002,
            ..Default::default()
        })
        .unwrap_err();
    assert!(
        matches!(err, RepoError::IntentBaseMismatch { .. }),
        "{err:?}"
    );

    // 不给 base ⇒ 自动填成父提交的树
    let second = repo
        .commit(CommitRequest {
            message: "v2".to_string(),
            session: "qoder:s1".to_string(),
            claims,
            intent: Some(Intent::new("改 v2", vec!["v2 在位".to_string()])),
            now_secs: 1_003,
            ..Default::default()
        })
        .unwrap();
    let commit = repo.commit_at(&second.id).unwrap();
    assert_eq!(commit.intent.unwrap().base, Some(first_tree));
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn witness_must_bind_the_exact_staged_tree() {
    let root = temp("witness-tree");
    let mut repo = Repo::init(&root).unwrap();
    let (tree, _) = repo.stage(&files(&[("a.txt", "alpha")]), 1_000).unwrap();
    let witness = run_witness(tree, None, 0);
    // 见证之后又改暂存内容 ⇒ 树变了 ⇒ 提交必须拒
    let (new_tree, _) = repo.stage(&files(&[("a.txt", "beta")]), 1_001).unwrap();
    assert_ne!(tree, new_tree);
    let err = repo
        .commit(CommitRequest {
            message: "stale witness".to_string(),
            session: "qoder:s1".to_string(),
            claims: vec![claim("qoder:s1", &["a.txt"], 9_999)],
            witness: Some(witness),
            now_secs: 1_002,
            ..Default::default()
        })
        .unwrap_err();
    assert!(
        matches!(
            err,
            RepoError::Witness(crate::witness::WitnessError::TreeMismatch { .. })
        ),
        "{err:?}"
    );
    assert!(repo.head().unwrap().is_none(), "错配的见证不许推进 HEAD");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn failing_witness_cannot_be_committed() {
    let root = temp("witness-fail");
    let mut repo = Repo::init(&root).unwrap();
    let (tree, _) = repo.stage(&files(&[("a.txt", "alpha")]), 1_000).unwrap();
    let err = repo
        .commit(CommitRequest {
            message: "red tests".to_string(),
            session: "qoder:s1".to_string(),
            claims: vec![claim("qoder:s1", &["a.txt"], 9_999)],
            witness: Some(run_witness(tree, None, 101)),
            now_secs: 1_000,
            ..Default::default()
        })
        .unwrap_err();
    assert!(matches!(err, RepoError::WitnessFailed(101)), "{err:?}");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn admission_rejects_a_tampered_witness_or_another_tree() {
    let root = temp("witness-admit");
    let mut repo = Repo::init(&root).unwrap();
    let (tree, _) = repo.stage(&files(&[("a.txt", "alpha")]), 1_000).unwrap();
    let bytes = run_witness(tree, None, 0);
    let outcome = repo
        .commit(CommitRequest {
            message: "witnessed".to_string(),
            session: "qoder:s1".to_string(),
            claims: vec![claim("qoder:s1", &["a.txt"], 9_999)],
            witness: Some(bytes.clone()),
            now_secs: 1_000,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        repo.commit_at(&outcome.id).unwrap().witness,
        Some(Hash::of(&bytes))
    );
    repo.admit(&outcome.id, &bytes).unwrap();
    // 只改一个字节：树绑定仍成立，但提交里记的文件摘要对不上
    let mut tampered = bytes.clone();
    let at = tampered
        .windows(9)
        .position(|window| window == b"trit-vcs/")
        .expect("见证里应有 tool 字段");
    tampered[at] = b'X';
    assert!(
        matches!(
            repo.admit(&outcome.id, &tampered).unwrap_err(),
            RepoError::Witness(crate::witness::WitnessError::FileTampered { .. })
        ),
        "改一个字节必须拒"
    );
    // 同一命令但对另一棵树 ⇒ 文件摘要先变，仍然拒
    let other = run_witness(Hash::of(b"other-tree"), None, 0);
    assert!(matches!(
        repo.admit(&outcome.id, &other).unwrap_err(),
        RepoError::Witness(crate::witness::WitnessError::FileTampered { .. })
    ));
    let report = repo.verify().unwrap();
    assert_eq!(report.witnessed, 1);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn admission_refuses_an_unwitnessed_commit() {
    let root = temp("witness-missing");
    let mut repo = Repo::init(&root).unwrap();
    let (tree, _) = repo.stage(&files(&[("a.txt", "alpha")]), 1_000).unwrap();
    let outcome = repo
        .commit(CommitRequest {
            message: "plain".to_string(),
            session: "qoder:s1".to_string(),
            claims: vec![claim("qoder:s1", &["a.txt"], 9_999)],
            now_secs: 1_000,
            ..Default::default()
        })
        .unwrap();
    assert!(matches!(
        repo.admit(&outcome.id, &run_witness(tree, None, 0))
            .unwrap_err(),
        RepoError::Unwitnessed(_)
    ));
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn query_filters_by_tag_evidence_session_and_message() {
    let root = temp("query");
    let mut repo = Repo::init(&root).unwrap();
    repo.stage(&files(&[("a.txt", "one")]), 1_000).unwrap();
    repo.commit(CommitRequest {
        message: "first fix".to_string(),
        session: "qoder:s1".to_string(),
        claims: vec![claim("qoder:s1", &["a.txt"], 9_999)],
        evidence: vec!["PR #1".to_string()],
        tags: vec!["vcs".to_string()],
        now_secs: 1_000,
        ..Default::default()
    })
    .unwrap();
    repo.stage(&files(&[("a.txt", "two")]), 2_000).unwrap();
    repo.commit(CommitRequest {
        message: "second".to_string(),
        session: "codex:c2".to_string(),
        claims: vec![claim("codex:c2", &["a.txt"], 9_999)],
        tags: vec!["gpu".to_string()],
        now_secs: 2_000,
        ..Default::default()
    })
    .unwrap();

    let count = |filter: CommitFilter| repo.query(&filter).unwrap().len();
    assert_eq!(
        count(CommitFilter {
            tag: Some("vcs".to_string()),
            ..Default::default()
        }),
        1
    );
    assert_eq!(
        count(CommitFilter {
            tag: Some("nope".to_string()),
            ..Default::default()
        }),
        0
    );
    assert_eq!(
        count(CommitFilter {
            evidence: Some("#1".to_string()),
            ..Default::default()
        }),
        1
    );
    assert_eq!(
        count(CommitFilter {
            session: Some("codex:c2".to_string()),
            ..Default::default()
        }),
        1
    );
    assert_eq!(
        count(CommitFilter {
            grep: Some("fix".to_string()),
            ..Default::default()
        }),
        1
    );
    assert_eq!(count(CommitFilter::default()), 2);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn merge_base_walks_to_common_ancestor() {
    let root = temp("base");
    let mut repo = Repo::init(&root).unwrap();
    let claims = vec![claim("qoder:s1", &["a.txt"], 9_999)];
    repo.stage(&files(&[("a.txt", "v1")]), 1_000).unwrap();
    let first = repo
        .commit(CommitRequest {
            message: "v1".to_string(),
            session: "qoder:s1".to_string(),
            claims: claims.clone(),
            now_secs: 1_000,
            ..Default::default()
        })
        .unwrap()
        .id;
    repo.stage(&files(&[("a.txt", "v2")]), 2_000).unwrap();
    let second = repo
        .commit(CommitRequest {
            message: "v2".to_string(),
            session: "qoder:s1".to_string(),
            claims: claims.clone(),
            now_secs: 2_000,
            ..Default::default()
        })
        .unwrap()
        .id;
    repo.stage(&files(&[("a.txt", "v3")]), 3_000).unwrap();
    let third = repo
        .commit(CommitRequest {
            message: "v3".to_string(),
            session: "qoder:s1".to_string(),
            claims,
            now_secs: 3_000,
            ..Default::default()
        })
        .unwrap()
        .id;
    assert_eq!(repo.merge_base(&second, &third).unwrap(), Some(second));
    assert_eq!(repo.merge_base(&first, &first).unwrap(), Some(first));
    let _ = fs::remove_dir_all(&root);
}
