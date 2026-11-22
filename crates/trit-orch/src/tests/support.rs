use crate::*;

pub(super) fn spec(id: &str) -> TaskSpec {
    TaskSpec {
        task: Task {
            id: id.into(),
            priority: 0,
            duration: 1,
            deps: Vec::new(),
        },
        requirements: Resources::new(1, 1),
        max_tokens: 10,
        epoch: 0,
    }
}

pub(super) fn edge(id: &str, required: Ternary) -> Dependency {
    Dependency {
        task: id.into(),
        required,
    }
}

pub(super) fn snapshot(tasks: &[TaskSpec]) -> Snapshot {
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

pub(super) fn limits() -> Limits {
    Limits {
        workers: 16,
        capacity: Resources::new(16, 16),
        tokens: 1_000,
    }
}
