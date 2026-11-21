//! 不变量检查器的验收：每条不变量都要能判红，且干净输入不许误报。
//!
//! 负例比正例重要：一个只会说"干净"的检查器是装饰，不是门禁。

use crate::*;

fn spec(id: &str) -> TaskSpec {
    TaskSpec {
        task: Task {
            id: id.into(),
            priority: 0,
            duration: 1,
            deps: Vec::new(),
        },
        requirements: Resources::new(1, 1),
        max_tokens: 100,
        epoch: 0,
    }
}

fn decision(
    task: &str,
    admission: Ternary,
    reasons: Vec<BlockReason>,
    obligations: Vec<AdmissionObligation>,
) -> Decision {
    Decision {
        task: task.into(),
        admission,
        reasons,
        obligations,
    }
}

fn selection(decisions: Vec<Decision>, selected: Vec<&str>) -> Selection {
    Selection {
        revision: 7,
        selected: selected.into_iter().map(str::to_owned).collect(),
        decisions,
        observed_tokens: TokenUsage::default(),
        reserved_tokens: 0,
        reserved_resources: Resources::new(0, 0),
        occupied_workers: 0,
    }
}

fn obligation(kind: ObligationKind, task: &str, subject: &str) -> AdmissionObligation {
    AdmissionObligation::new(kind, task, subject, "test")
}

fn report(result: CheckResult) -> CheckReport {
    CheckReport {
        epoch: 0,
        checks: 1,
        result,
    }
}

fn settled(task: &str, result: CheckResult) -> (TaskSpec, Observation) {
    (
        spec(task),
        Observation {
            phase: Phase::Settled,
            epoch: 0,
            authorization: Ternary::True,
            freshness: Ternary::True,
            evidence: Some(report(result)),
            usage: TokenUsage::default(),
        },
    )
}

fn snapshot_of(pairs: Vec<(TaskSpec, Observation)>) -> (Vec<TaskSpec>, Snapshot) {
    let mut tasks = Vec::new();
    let mut observations = std::collections::BTreeMap::new();
    for (spec, observation) in pairs {
        observations.insert(spec.task.id.clone(), observation);
        tasks.push(spec);
    }
    (
        tasks,
        Snapshot {
            revision: 7,
            observations,
        },
    )
}

// ---------------------------------------------------------------- 正例：干净输入零违例

#[test]
fn clean_selection_reports_nothing() {
    let report = check_selection(&selection(
        vec![decision("a", Ternary::True, Vec::new(), Vec::new())],
        vec!["a"],
    ));
    assert!(report.is_clean(), "{}", report.brief());
}

#[test]
fn status_only_decisions_are_not_unknown_without_obligation() {
    // 在途 / 已结算的 Unknown 是初始值，不是"待补证的未知"。
    let report = check_selection(&selection(
        vec![
            decision(
                "a",
                Ternary::Unknown,
                vec![BlockReason::InFlight],
                Vec::new(),
            ),
            decision(
                "b",
                Ternary::Unknown,
                vec![BlockReason::Settled],
                Vec::new(),
            ),
        ],
        Vec::new(),
    ));
    assert!(report.is_clean(), "{}", report.brief());
}

#[test]
fn conflict_with_adjudication_obligation_is_clean() {
    let report = check_selection(&selection(
        vec![decision(
            "a",
            Ternary::Unknown,
            vec![BlockReason::Conflict { task: "dep".into() }],
            vec![obligation(ObligationKind::Adjudicate, "a", "dep")],
        )],
        Vec::new(),
    ));
    assert!(report.is_clean(), "{}", report.brief());
}

// ---------------------------------------------------------------- 负例：每条都能判红

#[test]
fn unknown_without_obligation_goes_red() {
    let report = check_selection(&selection(
        vec![decision(
            "a",
            Ternary::Unknown,
            vec![BlockReason::Freshness {
                verdict: Ternary::Unknown,
            }],
            Vec::new(),
        )],
        Vec::new(),
    ));
    assert_eq!(report.counts()["unknown_without_obligation"], 1);
}

#[test]
fn conflict_without_adjudication_goes_red() {
    // 只补一条 Defer 也能让"无义务的 Unknown"过关，但冲突仍然必须单独判红。
    let report = check_selection(&selection(
        vec![decision(
            "a",
            Ternary::Unknown,
            vec![BlockReason::Conflict { task: "dep".into() }],
            vec![obligation(ObligationKind::Defer, "a", "dep")],
        )],
        Vec::new(),
    ));
    assert_eq!(report.counts()["conflict_without_adjudication"], 1);
    assert_eq!(
        report.violations[0].subject.as_deref(),
        Some("dep"),
        "违例必须指向真正的冲突源"
    );
}

#[test]
fn adjudication_on_the_wrong_subject_does_not_count() {
    let report = check_selection(&selection(
        vec![decision(
            "a",
            Ternary::Unknown,
            vec![BlockReason::Conflict { task: "dep".into() }],
            vec![obligation(ObligationKind::Adjudicate, "a", "other")],
        )],
        Vec::new(),
    ));
    assert_eq!(report.counts()["conflict_without_adjudication"], 1);
}

#[test]
fn decisive_admission_with_obligations_goes_red() {
    let report = check_selection(&selection(
        vec![decision(
            "a",
            Ternary::True,
            Vec::new(),
            vec![obligation(ObligationKind::Measure, "a", "x")],
        )],
        vec!["a"],
    ));
    assert_eq!(report.counts()["obligation_on_decisive"], 1);
}

#[test]
fn drifted_obligation_id_goes_red() {
    let mut drifted = obligation(ObligationKind::Measure, "a", "x");
    drifted.id = "measure:wrong".into();
    let report = check_selection(&selection(
        vec![decision("a", Ternary::Unknown, Vec::new(), vec![drifted])],
        Vec::new(),
    ));
    assert_eq!(report.counts()["obligation_id_drift"], 1);
}

#[test]
fn duplicate_obligation_goes_red() {
    let report = check_selection(&selection(
        vec![decision(
            "a",
            Ternary::Unknown,
            Vec::new(),
            vec![
                obligation(ObligationKind::Measure, "a", "x"),
                obligation(ObligationKind::Measure, "a", "x"),
            ],
        )],
        Vec::new(),
    ));
    assert_eq!(report.counts()["duplicate_obligation"], 1);
}

#[test]
fn selected_without_true_admission_goes_red() {
    let report = check_selection(&selection(
        vec![decision("a", Ternary::False, Vec::new(), Vec::new())],
        vec!["a"],
    ));
    assert_eq!(report.counts()["selected_without_admission"], 1);
}

#[test]
fn true_admission_but_not_selected_goes_red() {
    let report = check_selection(&selection(
        vec![decision("a", Ternary::True, Vec::new(), Vec::new())],
        Vec::new(),
    ));
    assert_eq!(report.counts()["admission_without_selection"], 1);
}

#[test]
fn selected_task_outside_decisions_goes_red() {
    let report = check_selection(&selection(
        vec![decision("a", Ternary::True, Vec::new(), Vec::new())],
        vec!["a", "ghost"],
    ));
    assert_eq!(report.counts()["selected_without_admission"], 1);
    assert_eq!(report.violations[0].task.as_deref(), Some("ghost"));
}

// ---------------------------------------------------------------- 完成闸门

#[test]
fn finish_claim_with_pending_task_goes_red() {
    let (tasks, snapshot) = snapshot_of(vec![
        settled("a", CheckResult::Passed),
        (
            spec("b"),
            Observation {
                phase: Phase::Pending,
                epoch: 0,
                authorization: Ternary::True,
                freshness: Ternary::True,
                evidence: None,
                usage: TokenUsage::default(),
            },
        ),
    ]);
    let report = check_finish_claim(&tasks, &snapshot);
    assert_eq!(report.counts()["finish_with_undetermined"], 1);
    assert!(
        report.violations[0].detail.contains("pending=1"),
        "证书要带具体计数：{}",
        report.violations[0].detail
    );
}

#[test]
fn finish_claim_with_conflict_goes_red() {
    let (tasks, snapshot) = snapshot_of(vec![settled("a", CheckResult::Conflicted)]);
    let report = check_finish_claim(&tasks, &snapshot);
    assert_eq!(report.counts()["finish_with_undetermined"], 1);
    assert!(report.violations[0].detail.contains("conflicts=1"));
}

#[test]
fn finish_claim_with_stale_epoch_goes_red() {
    // 结果属于旧定义：不可信，算未定。
    let (mut spec_a, mut obs_a) = settled("a", CheckResult::Passed);
    spec_a.epoch = 3;
    obs_a.epoch = 0;
    let (tasks, snapshot) = snapshot_of(vec![(spec_a, obs_a)]);
    let report = check_finish_claim(&tasks, &snapshot);
    assert_eq!(report.counts()["finish_with_undetermined"], 1);
}

#[test]
fn finish_claim_clean_when_everything_decisive() {
    let (tasks, snapshot) = snapshot_of(vec![
        settled("a", CheckResult::Passed),
        settled("b", CheckResult::Refuted),
    ]);
    assert!(check_finish_claim(&tasks, &snapshot).is_clean());
}

// ---------------------------------------------------------------- 契约与入口

#[test]
fn invariant_names_are_unique_snake_case() {
    let mut seen = std::collections::BTreeSet::new();
    for invariant in Invariant::ALL {
        let name = invariant.as_str();
        assert!(
            name.chars()
                .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit()),
            "`{name}` 不是 snake_case"
        );
        assert!(seen.insert(name), "`{name}` 重复");
    }
    assert_eq!(seen.len(), Invariant::ALL.len());
}

#[test]
fn assert_clean_is_the_ci_entry() {
    let clean = check_selection(&selection(
        vec![decision("a", Ternary::True, Vec::new(), Vec::new())],
        vec!["a"],
    ));
    assert!(clean.assert_clean().is_ok());

    let dirty = check_selection(&selection(
        vec![decision("a", Ternary::Unknown, Vec::new(), Vec::new())],
        Vec::new(),
    ));
    let err = dirty.clone().assert_clean().unwrap_err();
    assert_eq!(err.len(), dirty.len(), "判红必须保留全部证书");
}

#[test]
fn report_counts_and_brief_are_greppable() {
    let mut report = InvariantReport::default();
    report.merge(check_selection(&selection(
        vec![decision("a", Ternary::Unknown, Vec::new(), Vec::new())],
        Vec::new(),
    )));
    let brief = report.brief();
    assert!(brief.starts_with("invariants: "), "{brief}");
    assert!(brief.contains("unknown_without_obligation=1"), "{brief}");
}
