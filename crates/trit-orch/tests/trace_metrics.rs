use trit_orch::{
    Attempt, DeliveryReceipt, Effect, ReceiptAction, ReceiptState, TraceEvent, TraceEventKind,
    TraceLog, TracePayload, TraceRecorder,
};

fn attempt() -> Attempt {
    Attempt {
        operation: "pay".into(),
        task: "checkout".into(),
        epoch: 0,
        attempt: 1,
        fence: 1,
    }
}

#[test]
fn metrics_keep_event_time_and_observation_latency_separate() {
    let attempt = attempt();
    let mut prepared = TraceEvent::new("t1", 1, TraceEventKind::EffectPrepared, "pay", 100)
        .with_payload(TracePayload::Effect {
            attempt: attempt.clone(),
            state: Effect::Prepared,
        })
        .with_event_time(50);
    prepared.recorded_at = 95; // 写入早于观测：时钟回拨/宿主抖动
    let mut confirmed = TraceEvent::new("t1", 2, TraceEventKind::EffectConfirmed, "pay", 130)
        .with_payload(TracePayload::Effect {
            attempt,
            state: Effect::Confirmed,
        })
        .with_event_time(100);
    confirmed.recorded_at = 135;

    let mut log = TraceLog::new("t1");
    log.append(prepared).unwrap();
    log.append(confirmed).unwrap();
    let metrics = log.metrics(200);

    assert_eq!(metrics.events, 2);
    assert_eq!(
        metrics.effect_latency_event_time,
        Some(50),
        "事实时间：100 - 50"
    );
    assert_eq!(
        metrics.effect_latency_observed,
        Some(30),
        "观测时间：130 - 100，不能和事实时间混用"
    );
    assert_eq!(metrics.observation_span, Some(30));
    assert_eq!(metrics.trace_lag_max, Some(5));
    assert_eq!(metrics.clock_skew_events, 1, "recorded_at < observed_at");
    assert_eq!(metrics.event_time_missing, 0);
}

#[test]
fn metrics_report_in_doubt_age_and_missing_event_time() {
    let attempt = attempt();
    let mut log = TraceLog::new("t1");
    log.append(
        TraceEvent::new("t1", 1, TraceEventKind::EffectPrepared, "pay", 100).with_payload(
            TracePayload::Effect {
                attempt: attempt.clone(),
                state: Effect::Prepared,
            },
        ),
    )
    .unwrap();
    log.append(
        TraceEvent::new("t1", 2, TraceEventKind::EffectUnknown, "pay", 150).with_payload(
            TracePayload::Effect {
                attempt,
                state: Effect::Unknown,
            },
        ),
    )
    .unwrap();

    let metrics = log.metrics(200);
    assert_eq!(metrics.in_doubt, 1);
    assert_eq!(metrics.in_doubt_age_max, Some(50), "在途年龄按观测时间算");
    assert_eq!(metrics.event_time_missing, 2);
    assert_eq!(metrics.effect_latency_event_time, None);
    assert_eq!(metrics.effect_latency_observed, None);
    assert_eq!(metrics.observation_span, Some(50));
}

#[test]
fn metrics_compute_handoff_latency_from_receipt_events() {
    let sent = DeliveryReceipt::sent("r1", "k1").unwrap();
    let mut acted = sent.clone();
    acted.advance(ReceiptState::Delivered).unwrap();
    acted.advance(ReceiptState::Read).unwrap();
    acted.advance(ReceiptState::Acted).unwrap();

    let mut recorder = TraceRecorder::new("t1", 1);
    let mut log = TraceLog::new("t1");
    log.append(recorder.receipt(ReceiptAction::Recorded, sent, 100))
        .unwrap();
    log.append(recorder.receipt(ReceiptAction::Advanced, acted, 160))
        .unwrap();

    let metrics = log.metrics(200);
    assert_eq!(
        metrics.handoff_latency_observed,
        Some(60),
        "Recorded -> Acted 的观测延迟"
    );
    assert_eq!(metrics.selections, 0);
    assert_eq!(metrics.in_doubt, 0);
    assert_eq!(metrics.event_time_missing, 2);
}
