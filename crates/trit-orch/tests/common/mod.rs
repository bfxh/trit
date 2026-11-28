//! 轨迹域测试共用夹具：种子化生成器（SplitMix64）、合法事件流与单点违反注入。
//!
//! 合法性由构造保证；注入保证**恰好破一处**不变量，其余五条不动——这是
//! "注入必被抓、且报的就是注入那条"属性测试的地基。
//!
//! 每个测试二进制各自编译本模块，用不到的夹具不是死代码，按惯例整模块放开。
#![allow(dead_code)]

use std::path::PathBuf;

use trit_orch::{
    Ledger, Limits, Observation, Phase, Resources, Snapshot, Task, TaskSpec, Ternary, TraceEvent,
    TraceEventKind, TraceInvariant, TraceLog, TracePayload, TraceReport, TraceStep, TraceViolation,
};

pub fn temp_path(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "trit-orch-trace-inv-{tag}-{}.jsonl",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

pub fn spec(id: &str) -> TaskSpec {
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

pub fn snapshot(revision: u64, ids: &[&str]) -> Snapshot {
    Snapshot {
        revision,
        observations: ids
            .iter()
            .map(|id| {
                (
                    (*id).to_string(),
                    Observation {
                        phase: Phase::Pending,
                        authorization: Ternary::True,
                        freshness: Ternary::True,
                        ..Observation::default()
                    },
                )
            })
            .collect(),
    }
}

pub fn limits() -> Limits {
    Limits {
        workers: 2,
        capacity: Resources::new(2, 2),
        tokens: 1_000,
    }
}

pub fn step(revision: u64, ids: &[&str]) -> TraceStep {
    TraceStep {
        revision,
        tasks: ids.iter().map(|id| spec(id)).collect(),
        snapshot: snapshot(revision, ids),
        limits: limits(),
        ledger: Ledger::new(),
    }
}

/// 手写 SplitMix64：种子固定 ⇒ 序列固定，测试可复现。
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }
}

const LIGHT_KINDS: [TraceEventKind; 8] = [
    TraceEventKind::TaskSelected,
    TraceEventKind::EffectSent,
    TraceEventKind::ClaimAcquired,
    TraceEventKind::ReceiptAdvanced,
    TraceEventKind::ObligationScheduled,
    TraceEventKind::HandoffGated,
    TraceEventKind::CompactionDecided,
    TraceEventKind::TaskHeld,
];

/// 合法事件流生成器：n 条**轻事件** + 收尾的 plan/snapshot 对齐对。
///
/// 合法性由构造保证：seq 从 1 连续、`observed_at` 沿 seq 单调、`event_time` 不晚于
/// 获知（或 None）、`recorded_at` == 获知、因果只指向更早的既有事件、快照始终对齐
/// 当时的计划 revision（末尾对齐对保证折叠对齐）。
pub fn valid_events(seed: u64, trace: &str, n: u64) -> Vec<TraceEvent> {
    let mut rng = Rng::new(seed);
    let mut events = Vec::new();
    for seq in 1..=n {
        let observed_at = 1_000 + seq * 3;
        let kind = LIGHT_KINDS[rng.below(LIGHT_KINDS.len() as u64) as usize];
        let mut event = TraceEvent::new(trace, seq, kind, "corr", observed_at);
        if rng.below(10) < 6 {
            event.event_time = Some(observed_at - rng.below(6));
        }
        if rng.below(10) < 3 && seq > 1 {
            let earlier = 1 + rng.below(seq - 1);
            event.causation_id = Some(format!("{trace}:{earlier}"));
        }
        events.push(event);
    }
    // 收尾对齐对：折叠出的 plan/snapshot revision 必然相等。
    let plan_revision = 1 + seed % 97;
    let observed_at = 1_000 + (n + 1) * 3;
    events.push(
        TraceEvent::new(
            trace,
            n + 1,
            TraceEventKind::PlanPatched,
            "corr",
            observed_at,
        )
        .with_payload(TracePayload::Plan {
            revision: plan_revision,
            tasks: vec![spec("a")],
        }),
    );
    events.push(
        TraceEvent::new(
            trace,
            n + 2,
            TraceEventKind::SnapshotCaptured,
            "corr",
            observed_at + 1,
        )
        .with_payload(TracePayload::Snapshot {
            snapshot: snapshot(plan_revision, &["a"]),
        }),
    );
    events
}

/// 对合法事件流注入**恰好一处**违反；返回注入点的 seq。
pub fn inject(events: &mut Vec<TraceEvent>, invariant: TraceInvariant, seed: u64) -> u64 {
    let mut rng = Rng::new(seed ^ 0xCAFE);
    let last = events.len() - 1;
    match invariant {
        // 中段抽掉一个 seq：产生一个缺口区间；抽掉的事件不能在收尾对齐对里。
        TraceInvariant::SeqContiguous => {
            let victim = 1 + rng.below((events.len() - 3) as u64) as usize;
            let seq = events[victim].seq;
            let removed_id = events[victim].id.clone();
            events.remove(victim);
            // 指向被删事件的因果引用一并清掉：本条注入只允许破一个不变量。
            for event in events.iter_mut() {
                if event.causation_id.as_deref() == Some(removed_id.as_str()) {
                    event.causation_id = None;
                }
            }
            seq
        }
        TraceInvariant::CausationResolvable => {
            let victim = rng.below(events.len() as u64) as usize;
            let seq = events[victim].seq;
            events[victim].causation_id = Some(format!("ghost-{seq}"));
            seq
        }
        TraceInvariant::EventTimeNotAfterObservation => {
            let victim = rng.below(events.len() as u64) as usize;
            let seq = events[victim].seq;
            events[victim].event_time = Some(events[victim].observed_at + 7);
            seq
        }
        TraceInvariant::RecordedNotBeforeObservation => {
            let victim = rng.below(events.len() as u64) as usize;
            let seq = events[victim].seq;
            events[victim].recorded_at = events[victim].observed_at - 1;
            seq
        }
        TraceInvariant::HashChainLinked => {
            let victim = 1 + rng.below((events.len() - 1) as u64) as usize;
            let seq = events[victim].seq;
            events[victim].prev_hash = "tampered".into();
            seq
        }
        // 只翻最后一条快照的 revision：折叠对齐恰破一处。
        TraceInvariant::FoldRevisionAligned => {
            for index in (0..=last).rev() {
                if let Some(TracePayload::Snapshot { snapshot: state }) = &events[index].payload {
                    let seq = events[index].seq;
                    let mut drifted = state.clone();
                    drifted.revision += 1;
                    events[index].payload = Some(TracePayload::Snapshot { snapshot: drifted });
                    return seq;
                }
            }
            panic!("生成器保证至少有一条快照");
        }
    }
}

pub fn into_log(trace: &str, events: Vec<TraceEvent>) -> TraceLog {
    let mut log = TraceLog::new(trace);
    for event in events {
        log.append(event).expect("合法事件必须能入日志");
    }
    log
}

/// 断言报告恰好一条、且就是期望的那条不变量；返回该违例证书供位置断言。
pub fn expect_single(
    report: TraceReport,
    invariant: TraceInvariant,
    context: &str,
) -> TraceViolation {
    assert_eq!(report.len(), 1, "{context}: {report:?}");
    let violation = report.violations[0].clone();
    assert_eq!(violation.invariant, invariant, "{context}: {violation:?}");
    violation
}

pub fn serde_json_lines(events: &[TraceEvent]) -> Vec<String> {
    events
        .iter()
        .map(|event| serde_json::to_string(event).expect("序列化"))
        .collect()
}
