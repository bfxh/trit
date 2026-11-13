use trit_orch::{
    AdmissionObligation, BudgetBook, BudgetClass, BudgetError, BudgetSplit, ObligationKind,
    ObligationSkipReason, schedule_obligations_with_budget,
};

#[test]
fn pools_are_independent_and_failed_charge_is_atomic() {
    let mut book = BudgetBook::new(BudgetSplit {
        execution: 100,
        verification: 50,
        reconciliation: 20,
        human: 1,
    });
    book.charge(BudgetClass::Execution, 100).unwrap();
    book.charge(BudgetClass::Verification, 50).unwrap();
    book.charge(BudgetClass::Reconciliation, 20).unwrap();

    assert_eq!(book.remaining(BudgetClass::Execution), 0);
    assert_eq!(book.remaining(BudgetClass::Verification), 0);
    assert_eq!(book.remaining(BudgetClass::Reconciliation), 0);

    let before = book.report();
    assert!(matches!(
        book.try_charge(BudgetClass::Verification, 1, 1),
        Err(BudgetError::Tokens {
            class: BudgetClass::Verification,
            ..
        })
    ));
    assert_eq!(book.report(), before, "失败扣费不能部分生效");
    assert_eq!(book.human_remaining(), 1, "验证池耗尽不影响人的预算");
}

#[test]
fn human_failure_does_not_charge_tokens() {
    let mut book = BudgetBook::new(BudgetSplit {
        execution: 0,
        verification: 1_000,
        reconciliation: 0,
        human: 0,
    });
    let before = book.report();
    assert!(matches!(
        book.try_charge(BudgetClass::Verification, 100, 1),
        Err(BudgetError::Human { .. })
    ));
    assert_eq!(book.report(), before);
}

#[test]
fn verification_and_reconciliation_pools_are_charged_by_kind() {
    let measure =
        AdmissionObligation::new(ObligationKind::Measure, "a", "dep", "补证").with_deadline(100);
    let reconcile = AdmissionObligation::new(ObligationKind::Reconcile, "pay", "pay", "对账")
        .with_deadline(200);
    let mut book = BudgetBook::new(BudgetSplit {
        execution: 0,
        verification: 100,
        reconciliation: 300,
        human: 0,
    });

    let batch = schedule_obligations_with_budget(vec![measure, reconcile], 50, &mut book);
    assert_eq!(batch.scheduled.len(), 1);
    assert_eq!(batch.scheduled[0].kind, ObligationKind::Reconcile);
    assert_eq!(batch.budget.verification_spent, 0);
    assert_eq!(batch.budget.reconciliation_spent, 200);
    assert!(batch.skipped.iter().any(|skip| matches!(
        skip.reason,
        ObligationSkipReason::OverBudget {
            required: 500,
            available: 100
        }
    )));
}

#[test]
fn reconciliation_exhaustion_does_not_block_verification() {
    let reconcile = AdmissionObligation::new(ObligationKind::Reconcile, "pay", "pay", "对账")
        .with_deadline(100);
    let measure =
        AdmissionObligation::new(ObligationKind::Measure, "a", "dep", "补证").with_deadline(200);
    let mut book = BudgetBook::new(BudgetSplit {
        execution: 0,
        verification: 500,
        reconciliation: 100,
        human: 0,
    });

    let batch = schedule_obligations_with_budget(vec![reconcile, measure], 50, &mut book);
    assert_eq!(batch.scheduled.len(), 1);
    assert_eq!(batch.scheduled[0].kind, ObligationKind::Measure);
    assert_eq!(batch.budget.reconciliation_spent, 0);
    assert_eq!(batch.budget.verification_spent, 500);
}

#[test]
fn human_budget_skips_escalation_but_not_measure() {
    let escalate = AdmissionObligation::new(ObligationKind::Escalate, "h", "auth", "人决策")
        .with_deadline(100);
    let escalate_id = escalate.id.clone();
    let measure =
        AdmissionObligation::new(ObligationKind::Measure, "a", "dep", "补证").with_deadline(200);
    let mut book = BudgetBook::new(BudgetSplit {
        execution: 0,
        verification: 500,
        reconciliation: 0,
        human: 0,
    });

    let batch = schedule_obligations_with_budget(vec![escalate, measure], 50, &mut book);
    assert_eq!(batch.scheduled.len(), 1);
    assert_eq!(batch.scheduled[0].kind, ObligationKind::Measure);
    assert!(
        batch
            .skipped
            .iter()
            .any(|skip| skip.id == escalate_id && skip.reason == ObligationSkipReason::NoHuman)
    );
    assert_eq!(batch.budget.human_spent, 0);
}

#[test]
fn execution_pool_does_not_pay_for_obligations() {
    let measure =
        AdmissionObligation::new(ObligationKind::Measure, "a", "dep", "补证").with_deadline(100);
    let mut book = BudgetBook::new(BudgetSplit {
        execution: 1_000,
        verification: 0,
        reconciliation: 0,
        human: 0,
    });

    let batch = schedule_obligations_with_budget(vec![measure], 50, &mut book);
    assert!(batch.scheduled.is_empty());
    assert_eq!(book.remaining(BudgetClass::Execution), 1_000);
}
