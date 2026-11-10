//! 长上下文压缩闸门的判定测试。每个测试只判一件事，断言串供变异门定位。

use trit_orch::{
    CompactionBlock, CompactionDecision, ContextFacts, MAX_THRESHOLD_BP, MIN_THRESHOLD_BP,
    Obligation, Overflow, Ternary, Tier, compaction_gate,
};

const WINDOW: u64 = 100_000;
/// 0.51 -> 阈值 51_000；测到 60_000 时超出 9_000。
const THRESHOLD_BP: u32 = 5_100;
const OVER: u64 = 60_000;

fn pressure() -> ContextFacts {
    ContextFacts {
        measured_tokens: Some(OVER),
        context_window: WINDOW,
        threshold_bp: THRESHOLD_BP,
        prunable_tokens: 0,
        overflow: Overflow::NotReported,
        durable_flushed: Ternary::True,
        recall_reachable: Ternary::True,
        constraints_pinned: Ternary::True,
        progress_since_compaction: Ternary::True,
    }
}

fn gate(facts: ContextFacts) -> CompactionDecision {
    compaction_gate(&facts).unwrap_or_else(|error| panic!("ORCH_CTX_CONFIG: 夹具不该判红: {error}"))
}

#[test]
fn below_threshold_does_not_compact() {
    let facts = ContextFacts {
        measured_tokens: Some(1_000),
        ..pressure()
    };
    let decision = gate(facts);
    assert_eq!(decision.tier, Tier::Keep, "ORCH_CTX_BELOW");
    assert_eq!(decision.admission, Ternary::False, "ORCH_CTX_BELOW");
    assert_eq!(decision.need, Ternary::False, "ORCH_CTX_BELOW");
    assert_eq!(
        decision.blocks,
        vec![CompactionBlock::BelowThreshold],
        "ORCH_CTX_BELOW"
    );
}

#[test]
fn sufficient_prunable_with_proved_recall_uses_prune_not_summary() {
    // 摘要门全是否证：仍应走剪枝，证明低档不借高档的凭证。
    let facts = ContextFacts {
        prunable_tokens: 9_000,
        durable_flushed: Ternary::False,
        constraints_pinned: Ternary::False,
        ..pressure()
    };
    let decision = gate(facts);
    assert_eq!(decision.tier, Tier::Prune, "ORCH_CTX_PRUNE_FIRST");
    assert_eq!(decision.admission, Ternary::True, "ORCH_CTX_PRUNE_FIRST");
    assert_eq!(decision.excess_tokens, 9_000, "ORCH_CTX_PRUNE_FIRST");
    assert!(decision.blocks.is_empty(), "ORCH_CTX_PRUNE_FIRST");
}

#[test]
fn insufficient_prunable_falls_to_summary_gate() {
    let facts = ContextFacts {
        prunable_tokens: 100,
        ..pressure()
    };
    let decision = gate(facts);
    assert_eq!(decision.tier, Tier::Summarize, "ORCH_CTX_SUMMARIZE");
    assert_eq!(decision.admission, Ternary::True, "ORCH_CTX_SUMMARIZE");
}

#[test]
fn zero_prunable_never_counts_as_relief() {
    // 恰好等于阈值：超出量为 0，但零可剪枝量不等于「剪枝够了」。
    let facts = ContextFacts {
        measured_tokens: Some(OVER - 9_000),
        prunable_tokens: 0,
        ..pressure()
    };
    let decision = gate(facts);
    assert_eq!(decision.excess_tokens, 0, "ORCH_CTX_ZERO_PRUNABLE");
    assert_eq!(decision.tier, Tier::Summarize, "ORCH_CTX_ZERO_PRUNABLE");
}

#[test]
fn unknown_recall_does_not_escalate_to_lossy_tier() {
    let facts = ContextFacts {
        prunable_tokens: 9_000,
        recall_reachable: Ternary::Unknown,
        ..pressure()
    };
    let decision = gate(facts);
    assert_eq!(
        decision.tier,
        Tier::Keep,
        "ORCH_CTX_NO_ESCALATE_ON_UNKNOWN_RECALL"
    );
    assert_eq!(
        decision.admission,
        Ternary::Unknown,
        "ORCH_CTX_NO_ESCALATE_ON_UNKNOWN_RECALL"
    );
    assert_eq!(
        decision.blocks,
        vec![CompactionBlock::RecallUnreachable],
        "ORCH_CTX_NO_ESCALATE_ON_UNKNOWN_RECALL"
    );
    assert!(
        decision.obligations.contains(&Obligation::VerifyRecall),
        "ORCH_CTX_NO_ESCALATE_ON_UNKNOWN_RECALL"
    );
}

#[test]
fn refuted_recall_hands_over_to_summary_gates() {
    let facts = ContextFacts {
        prunable_tokens: 9_000,
        recall_reachable: Ternary::False,
        ..pressure()
    };
    let decision = gate(facts);
    assert_eq!(
        decision.tier,
        Tier::Summarize,
        "ORCH_CTX_RECALL_REFUTED_FALLS_THROUGH"
    );
    assert_eq!(
        decision.admission,
        Ternary::True,
        "ORCH_CTX_RECALL_REFUTED_FALLS_THROUGH"
    );
}

#[test]
fn unflushed_facts_block_lossy_summary() {
    let facts = ContextFacts {
        durable_flushed: Ternary::Unknown,
        ..pressure()
    };
    let decision = gate(facts);
    assert_eq!(decision.tier, Tier::Keep, "ORCH_CTX_FLUSH_GATE");
    assert_eq!(decision.admission, Ternary::Unknown, "ORCH_CTX_FLUSH_GATE");
    assert_eq!(
        decision.blocks,
        vec![CompactionBlock::FactsNotFlushed],
        "ORCH_CTX_FLUSH_GATE"
    );
    assert_eq!(
        decision.obligations,
        vec![Obligation::FlushFacts],
        "ORCH_CTX_FLUSH_GATE"
    );
}

#[test]
fn unpinned_constraints_refuses_and_names_the_fix() {
    let facts = ContextFacts {
        constraints_pinned: Ternary::False,
        ..pressure()
    };
    let decision = gate(facts);
    assert_eq!(decision.tier, Tier::Keep, "ORCH_CTX_PIN_GATE");
    assert_eq!(decision.admission, Ternary::False, "ORCH_CTX_PIN_GATE");
    assert_eq!(
        decision.need,
        Ternary::True,
        "ORCH_CTX_PIN_GATE: 压力判定不该被安全门改写"
    );
    assert_eq!(
        decision.blocks,
        vec![CompactionBlock::ConstraintsUnpinned],
        "ORCH_CTX_PIN_GATE"
    );
    assert_eq!(
        decision.obligations,
        vec![Obligation::PinConstraints],
        "ORCH_CTX_PIN_GATE"
    );
}

#[test]
fn compaction_without_new_durable_progress_is_refused() {
    let facts = ContextFacts {
        progress_since_compaction: Ternary::False,
        ..pressure()
    };
    let decision = gate(facts);
    assert_eq!(decision.tier, Tier::Keep, "ORCH_CTX_RHYTHM_GATE");
    assert_eq!(
        decision.blocks,
        vec![CompactionBlock::NoDurableProgress],
        "ORCH_CTX_RHYTHM_GATE"
    );
    assert_eq!(
        decision.obligations,
        vec![Obligation::RecheckAfterProgress],
        "ORCH_CTX_RHYTHM_GATE"
    );
}

#[test]
fn overflow_report_below_half_window_is_treated_as_provider_error() {
    let facts = ContextFacts {
        measured_tokens: Some(1_000),
        overflow: Overflow::Reported,
        ..pressure()
    };
    let decision = gate(facts);
    assert_eq!(decision.tier, Tier::Keep, "ORCH_CTX_FALSE_OVERFLOW");
    assert_eq!(
        decision.admission,
        Ternary::False,
        "ORCH_CTX_FALSE_OVERFLOW"
    );
    assert_eq!(
        decision.blocks,
        vec![CompactionBlock::SuspectedFalseOverflow],
        "ORCH_CTX_FALSE_OVERFLOW"
    );
}

#[test]
fn report_and_measurement_disagree_defers_to_window_reconcile() {
    let facts = ContextFacts {
        measured_tokens: Some(80_000),
        threshold_bp: 9_000,
        overflow: Overflow::Reported,
        ..pressure()
    };
    let decision = gate(facts);
    assert_eq!(
        decision.admission,
        Ternary::Unknown,
        "ORCH_CTX_REPORT_DISAGREES"
    );
    assert_eq!(
        decision.blocks,
        vec![CompactionBlock::ReportDisagrees],
        "ORCH_CTX_REPORT_DISAGREES"
    );
    assert_eq!(
        decision.obligations,
        vec![Obligation::ReconcileWindow],
        "ORCH_CTX_REPORT_DISAGREES"
    );
}

#[test]
fn unmeasured_pressure_is_unknown_not_absent() {
    let facts = ContextFacts {
        measured_tokens: None,
        ..pressure()
    };
    let decision = gate(facts);
    assert_eq!(decision.need, Ternary::Unknown, "ORCH_CTX_UNMEASURED");
    assert_eq!(decision.tier, Tier::Keep, "ORCH_CTX_UNMEASURED");
    assert_eq!(
        decision.blocks,
        vec![CompactionBlock::UnmeasuredPressure],
        "ORCH_CTX_UNMEASURED"
    );
    assert_eq!(
        decision.obligations,
        vec![Obligation::MeasureTokens],
        "ORCH_CTX_UNMEASURED"
    );
}

#[test]
fn overflow_report_without_measurement_does_not_approve_compaction() {
    // 供应商报错是可重试的，改写历史不是；测不到就不批准。
    let facts = ContextFacts {
        measured_tokens: None,
        overflow: Overflow::Reported,
        ..pressure()
    };
    let decision = gate(facts);
    assert_eq!(
        decision.admission,
        Ternary::Unknown,
        "ORCH_CTX_OVERFLOW_UNMEASURED_NOT_APPROVED"
    );
    assert_eq!(
        decision.tier,
        Tier::Keep,
        "ORCH_CTX_OVERFLOW_UNMEASURED_NOT_APPROVED"
    );
}

#[test]
fn default_facts_leave_every_safety_gate_unknown() {
    let facts = ContextFacts::default();
    assert_eq!(
        facts.durable_flushed,
        Ternary::Unknown,
        "ORCH_CTX_DEFAULT_UNKNOWN"
    );
    assert_eq!(
        facts.recall_reachable,
        Ternary::Unknown,
        "ORCH_CTX_DEFAULT_UNKNOWN"
    );
    assert_eq!(
        facts.constraints_pinned,
        Ternary::Unknown,
        "ORCH_CTX_DEFAULT_UNKNOWN"
    );
    assert_eq!(
        facts.progress_since_compaction,
        Ternary::Unknown,
        "ORCH_CTX_DEFAULT_UNKNOWN"
    );
    assert_eq!(facts.measured_tokens, None, "ORCH_CTX_DEFAULT_UNKNOWN");
    assert_eq!(
        compaction_gate(&facts).map(|_| ()),
        Err(trit_orch::Error::ContextConfig {
            message: "context_window 必须大于 0，否则阈值判定没有分母".into()
        }),
        "ORCH_CTX_ZERO_WINDOW"
    );
}

#[test]
fn threshold_out_of_bounds_is_config_error_with_stable_kind() {
    for bp in [MIN_THRESHOLD_BP - 1, MAX_THRESHOLD_BP + 1] {
        let facts = ContextFacts {
            threshold_bp: bp,
            ..pressure()
        };
        let error = compaction_gate(&facts)
            .map(|_| ())
            .expect_err("ORCH_CTX_BP_BOUNDS: 越界阈值本该判红");
        assert_eq!(error.kind(), "context_config", "ORCH_CTX_BP_BOUNDS");
    }
}

#[test]
fn refuted_safety_gates_refute_approval_not_just_defer_it() {
    // need 为 True、剪枝不够用、三道门全被否证：结论必须是 False（明确不做），不是 Unknown（待补证）。
    let facts = ContextFacts {
        prunable_tokens: 100,
        durable_flushed: Ternary::False,
        constraints_pinned: Ternary::False,
        progress_since_compaction: Ternary::False,
        ..pressure()
    };
    let decision = gate(facts);
    assert_eq!(decision.need, Ternary::True, "ORCH_CTX_REFUTED_GATES");
    assert_eq!(
        decision.admission,
        Ternary::False,
        "ORCH_CTX_REFUTED_GATES: 否证的安全门不该退化成待定"
    );
    assert_eq!(decision.tier, Tier::Keep, "ORCH_CTX_REFUTED_GATES");
    assert_eq!(
        decision.blocks,
        vec![
            CompactionBlock::FactsNotFlushed,
            CompactionBlock::ConstraintsUnpinned,
            CompactionBlock::NoDurableProgress
        ],
        "ORCH_CTX_REFUTED_GATES"
    );
}

#[test]
fn summary_gates_follow_kleene_conjunction_over_all_27_combinations() {
    let values = [Ternary::False, Ternary::Unknown, Ternary::True];
    let mut approved = 0_usize;
    for flush in values {
        for pin in values {
            for progress in values {
                let facts = ContextFacts {
                    // prunable 为 0，剪枝档永不成立，这里只扫摘要的三道门。
                    durable_flushed: flush,
                    constraints_pinned: pin,
                    progress_since_compaction: progress,
                    ..pressure()
                };
                let decision = gate(facts);
                let expected = flush & pin & progress;
                assert_eq!(
                    decision.admission, expected,
                    "ORCH_CTX_K3_TABLE: 合取值不符 {flush} {pin} {progress}"
                );
                let expected_blocks = [flush, pin, progress]
                    .iter()
                    .filter(|verdict| **verdict != Ternary::True)
                    .count();
                assert_eq!(
                    decision.blocks.len(),
                    expected_blocks,
                    "ORCH_CTX_K3_TABLE: 缺证项数不符 {flush} {pin} {progress}"
                );
                if decision.tier != Tier::Keep {
                    approved += 1;
                    assert_eq!(
                        decision.admission,
                        Ternary::True,
                        "ORCH_CTX_K3_TABLE: 未批准却动刀"
                    );
                    assert_eq!(
                        decision.tier,
                        Tier::Summarize,
                        "ORCH_CTX_K3_TABLE: 档位不符"
                    );
                }
            }
        }
    }
    assert_eq!(approved, 1, "ORCH_CTX_K3_TABLE: 27 组里只有三真可批准");
}
