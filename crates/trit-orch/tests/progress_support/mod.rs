use trit_orch::*;

pub(crate) fn task(id: &str) -> TaskSpec {
    TaskSpec {
        task: Task {
            id: id.into(),
            priority: 0,
            duration: 1,
            deps: vec![],
        },
        requirements: Resources::new(1, 1),
        max_tokens: 100,
        policy: trit_orch::TaskPolicy::default(),
    }
}

pub(crate) fn snapshot(tasks: &[TaskSpec]) -> Snapshot {
    Snapshot {
        revision: 1,
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

pub(crate) fn passed() -> CheckReport {
    CheckReport {
        epoch: 0,
        checks: 1,
        result: CheckResult::Passed,
    }
}
