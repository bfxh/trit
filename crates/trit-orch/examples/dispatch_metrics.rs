use std::collections::BTreeMap;
use std::error::Error;
use std::time::Instant;

use trit_orch::{
    CheckReport, CheckResult, Dependency, Limits, Observation, Phase, Plan, Resources, Snapshot,
    Task, TaskSpec, Ternary, TokenUsage,
};

fn fixture(count: usize) -> (Vec<TaskSpec>, Snapshot) {
    let mut tasks = Vec::with_capacity(count);
    let mut observations = BTreeMap::new();
    for i in 0..count {
        let id = format!("task-{i:05}");
        let deps = if i > 0 && i % 5 == 0 {
            vec![Dependency {
                task: format!("task-{:05}", i - 1),
                required: Ternary::True,
            }]
        } else {
            vec![]
        };
        tasks.push(TaskSpec {
            task: Task {
                id: id.clone(),
                priority: (i % 3) as i32,
                duration: 1 + (i % 5) as u32,
                deps,
            },
            requirements: Resources::new(1, 8),
            max_tokens: 1024,
        });
        let mut observation = Observation {
            authorization: Ternary::True,
            freshness: Ternary::True,
            ..Observation::default()
        };
        if i == 0 {
            observation.phase = Phase::InFlight;
            observation.usage = TokenUsage {
                fresh: 120,
                cached: 780,
                output: 40,
            };
        } else if i % 7 == 0 {
            observation.phase = Phase::Settled;
            observation.evidence = Some(CheckReport {
                revision: 1,
                checks: 1,
                result: CheckResult::Passed,
            });
            observation.usage = TokenUsage {
                fresh: 100,
                cached: 300,
                output: 50,
            };
        }
        observations.insert(id, observation);
    }
    (
        tasks,
        Snapshot {
            revision: 1,
            observations,
        },
    )
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let count = args.next().map(|s| s.parse()).transpose()?.unwrap_or(1000);
    let rounds = args.next().map(|s| s.parse()).transpose()?.unwrap_or(20);
    if args.next().is_some() || !(4..=10_000).contains(&count) || !(1..=1000).contains(&rounds) {
        return Err("usage: dispatch_metrics [tasks:4..10000] [rounds:1..1000]".into());
    }
    let (tasks, snapshot) = fixture(count);
    let start = Instant::now();
    let plan = Plan::new(1, tasks)?;
    let plan_us = start.elapsed().as_micros();
    let limits = Limits {
        workers: 4,
        capacity: Resources::new(4, 32),
        tokens: count as u64 * 1024,
    };
    let expected = plan.select(&snapshot, &limits)?;
    let mut times = Vec::with_capacity(rounds);
    for _ in 0..rounds {
        let start = Instant::now();
        let selection = plan.select(&snapshot, &limits)?;
        times.push(start.elapsed().as_micros());
        assert_eq!(selection, expected, "ORCH_DETERMINISTIC_METRICS");
    }
    assert_eq!(expected.selected.len(), 3, "ORCH_FIXTURE_BATCH");
    assert_eq!(expected.occupied_workers, 4);
    assert!(expected.observed_tokens.total()? + expected.reserved_tokens <= limits.tokens);
    assert!(
        expected
            .reserved_resources
            .fits_in(limits.capacity, Resources::default())
    );
    times.sort_unstable();
    let counts = expected.counts();
    let (held, denied, inflight, settled) =
        (counts.held, counts.denied, counts.in_flight, counts.settled);
    println!(
        "{{\"source\":\"synthetic_not_agent_evaluation\",\"tasks\":{count},\"rounds\":{rounds},\"plan_us\":{plan_us},\"select_p50_us\":{},\"select_p95_us\":{},\"selected\":{},\"held\":{held},\"denied\":{denied},\"inflight\":{inflight},\"settled\":{settled},\"occupied_workers\":{},\"cpu_units\":{},\"memory_units\":{},\"fixture_fresh_tokens\":{},\"fixture_cached_tokens\":{},\"fixture_output_tokens\":{},\"reserved_tokens\":{},\"model_calls\":0,\"brief\":\"{}\"}}",
        times[(rounds - 1) / 2],
        times[(rounds * 95).div_ceil(100) - 1],
        expected.selected.len(),
        expected.occupied_workers,
        expected.reserved_resources.cpu,
        expected.reserved_resources.memory,
        expected.observed_tokens.fresh,
        expected.observed_tokens.cached,
        expected.observed_tokens.output,
        expected.reserved_tokens,
        expected.brief(),
    );
    Ok(())
}
