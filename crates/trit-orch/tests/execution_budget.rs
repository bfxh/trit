use trit_orch::{
    BudgetBook, BudgetClass, BudgetError, BudgetSplit, DispatchBudgetError, ExecutionRequest,
    Ledger, Limits, Observation, Plan, Resources, Snapshot, Task, TaskSpec, Ternary,
    dispatch_with_execution_budget, reserve_for_dispatch,
};

fn task(id: &str, max_tokens: u64) -> TaskSpec {
    TaskSpec {
        task: Task {
            id: id.into(),
            priority: 0,
            duration: 1,
            deps: Vec::new(),
        },
        requirements: Resources::new(1, 1),
        max_tokens,
        epoch: 0,
    }
}

fn snapshot(tasks: &[TaskSpec]) -> Snapshot {
    Snapshot {
        revision: 7,
        observations: tasks
            .iter()
            .map(|spec| {
                (
                    spec.task.id.clone(),
                    Observation {
                        authorization: Ternary::True,
                        freshness: Ternary::True,
                        ..Observation::default()
                    },
                )
            })
            .collect(),
    }
}

fn limits(workers: usize, tokens: u64) -> Limits {
    Limits {
        workers,
        capacity: Resources::new(100, 100),
        tokens,
    }
}

fn request(operation: &str, tokens: u64) -> ExecutionRequest {
    ExecutionRequest {
        operation: operation.into(),
        task: operation.into(),
        epoch: 0,
        tokens,
    }
}

#[test]
fn selection_grants_per_task_and_failed_batch_reservation_is_atomic() {
    let tasks = vec![task("a", 100), task("b", 50)];
    let state = snapshot(&tasks);
    let selection = Plan::new(7, tasks.clone())
        .unwrap()
        .select(&state, &limits(2, 1_000))
        .unwrap();
    assert_eq!(
        selection
            .decisions
            .iter()
            .find(|decision| decision.task == "a")
            .unwrap()
            .granted_tokens,
        100
    );
    assert_eq!(
        selection
            .decisions
            .iter()
            .find(|decision| decision.task == "b")
            .unwrap()
            .granted_tokens,
        50
    );
    assert_eq!(selection.reserved_tokens, 150);

    let mut book = BudgetBook::new(BudgetSplit {
        execution: 100,
        verification: 0,
        reconciliation: 0,
        human: 0,
    });
    assert!(matches!(
        reserve_for_dispatch(&selection, &tasks, &mut book),
        Err(BudgetError::Tokens {
            class: BudgetClass::Execution,
            required: 150,
            available: 100
        })
    ));
    assert_eq!(
        book.remaining(BudgetClass::Execution),
        100,
        "批量失败不能部分预留"
    );
}

#[test]
fn reserve_settle_and_release_execution_budget() {
    let tasks = vec![task("a", 100)];
    let state = snapshot(&tasks);
    let selection = Plan::new(7, tasks.clone())
        .unwrap()
        .select(&state, &limits(1, 1_000))
        .unwrap();
    let mut book = BudgetBook::new(BudgetSplit {
        execution: 1_000,
        verification: 0,
        reconciliation: 0,
        human: 0,
    });

    let reservations = reserve_for_dispatch(&selection, &tasks, &mut book).unwrap();
    assert_eq!(reservations.len(), 1);
    assert_eq!(book.remaining(BudgetClass::Execution), 900);

    let settlement = book.settle_execution("a", 70).unwrap();
    assert_eq!(settlement.reserved, 100);
    assert_eq!(settlement.used, 70);
    assert_eq!(settlement.released, 30);
    assert_eq!(settlement.overrun, 0);
    assert_eq!(book.remaining(BudgetClass::Execution), 930);
    assert_eq!(book.spent(BudgetClass::Execution), 70);
}

#[test]
fn settle_overrun_records_the_fact() {
    let mut book = BudgetBook::new(BudgetSplit {
        execution: 1_000,
        verification: 0,
        reconciliation: 0,
        human: 0,
    });
    book.reserve_batch([request("a", 100)]).unwrap();

    let settlement = book.settle_execution("a", 130).unwrap();
    assert_eq!(settlement.overrun, 30);
    assert_eq!(settlement.released, 0);
    assert_eq!(book.spent(BudgetClass::Execution), 130);
    assert_eq!(book.remaining(BudgetClass::Execution), 870);
}

#[test]
fn release_returns_reserved_budget_and_unknown_settle_fails() {
    let mut book = BudgetBook::new(BudgetSplit {
        execution: 1_000,
        verification: 0,
        reconciliation: 0,
        human: 0,
    });
    book.reserve_batch([request("a", 100)]).unwrap();
    assert_eq!(book.remaining(BudgetClass::Execution), 900);

    let released = book.release_execution("a").unwrap();
    assert_eq!(released.tokens, 100);
    assert_eq!(book.remaining(BudgetClass::Execution), 1_000);
    assert_eq!(book.spent(BudgetClass::Execution), 0);
    assert!(matches!(
        book.settle_execution("a", 10),
        Err(BudgetError::UnknownReservation { .. })
    ));
}

#[test]
fn duplicate_operations_are_rejected_without_reserving() {
    let mut book = BudgetBook::new(BudgetSplit {
        execution: 1_000,
        verification: 0,
        reconciliation: 0,
        human: 0,
    });
    assert!(matches!(
        book.reserve_batch([request("a", 100), request("a", 100)]),
        Err(BudgetError::DuplicateReservation { .. })
    ));
    assert_eq!(book.remaining(BudgetClass::Execution), 1_000);
}

#[test]
fn dispatch_with_budget_releases_reservations_when_dispatch_fails() {
    let tasks = vec![task("a", 100)];
    let state = snapshot(&tasks);
    let selection = Plan::new(7, tasks.clone())
        .unwrap()
        .select(&state, &limits(1, 1_000))
        .unwrap();
    let mut book = BudgetBook::new(BudgetSplit {
        execution: 1_000,
        verification: 0,
        reconciliation: 0,
        human: 0,
    });

    let error =
        dispatch_with_execution_budget(&selection, &tasks, &Ledger::new(), &mut book).unwrap_err();
    assert!(matches!(error, DispatchBudgetError::Dispatch(_)));
    assert_eq!(
        book.remaining(BudgetClass::Execution),
        1_000,
        "派发失败必须释放预留"
    );

    let mut ledger = Ledger::new();
    ledger.prepare("t", 0, "a", 1).unwrap();
    let (attempts, reservations) =
        dispatch_with_execution_budget(&selection, &tasks, &ledger, &mut book).unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(reservations.len(), 1);
    assert_eq!(book.remaining(BudgetClass::Execution), 900);
}

#[test]
fn two_tasks_settle_independently() {
    let mut book = BudgetBook::new(BudgetSplit {
        execution: 1_000,
        verification: 0,
        reconciliation: 0,
        human: 0,
    });
    book.reserve_batch([request("a", 100), request("b", 50)])
        .unwrap();
    assert_eq!(book.remaining(BudgetClass::Execution), 850);

    book.settle_execution("a", 80).unwrap();
    book.release_execution("b").unwrap();
    assert_eq!(book.spent(BudgetClass::Execution), 80);
    assert_eq!(book.remaining(BudgetClass::Execution), 920);
}
