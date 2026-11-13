use trit_orch::{EvidenceVerdict, LaneDispute, LaneDriver, LaneState, TriState, resolve_lanes};

fn driver(owner: &str, value: TriState) -> LaneDriver {
    LaneDriver {
        owner: owner.into(),
        value,
    }
}

#[test]
fn silent_lane_resolves_to_high_z() {
    let report = resolve_lanes([("lane".to_string(), Vec::new())]);
    assert_eq!(report.lanes["lane"], LaneState::Agreed(TriState::HighZ));
    assert!(report.conflicts.is_empty());
}

#[test]
fn same_values_agree_and_high_z_is_identity() {
    let report = resolve_lanes([(
        "lane".to_string(),
        vec![
            driver("alice", TriState::High),
            driver("bob", TriState::High),
            driver("carol", TriState::HighZ),
        ],
    )]);
    assert_eq!(report.lanes["lane"], LaneState::Agreed(TriState::High));
    assert!(report.conflicts.is_empty());
}

#[test]
fn opposite_values_conflict_and_list_all_active_drivers() {
    let report = resolve_lanes([(
        "lane".to_string(),
        vec![
            driver("alice", TriState::High),
            driver("bob", TriState::Low),
            driver("carol", TriState::HighZ),
            driver("dave", TriState::High),
        ],
    )]);
    let LaneState::Conflicted(dispute) = &report.lanes["lane"] else {
        panic!("必须判冲突");
    };
    assert_eq!(dispute.lane, "lane");
    assert_eq!(
        dispute
            .drivers
            .iter()
            .map(|driver| driver.owner.as_str())
            .collect::<Vec<_>>(),
        ["alice", "bob", "dave"],
        "HighZ 不参与，所有 active 驱动都要列出"
    );
    assert_eq!(report.conflicts.len(), 1);
}

#[test]
fn all_conflict_lanes_are_reported() {
    let report = resolve_lanes([
        (
            "a".to_string(),
            vec![
                driver("alice", TriState::High),
                driver("bob", TriState::Low),
            ],
        ),
        (
            "b".to_string(),
            vec![
                driver("alice", TriState::Low),
                driver("bob", TriState::High),
            ],
        ),
        (
            "c".to_string(),
            vec![
                driver("alice", TriState::High),
                driver("bob", TriState::High),
            ],
        ),
    ]);
    assert_eq!(report.conflicts.len(), 2);
    assert_eq!(report.lanes["c"], LaneState::Agreed(TriState::High));
    assert_eq!(
        report
            .conflicts
            .iter()
            .map(|dispute| dispute.lane.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
}

#[test]
fn lane_state_maps_to_evidence_verdict() {
    assert_eq!(
        LaneState::Agreed(TriState::High).evidence_verdict(),
        EvidenceVerdict::Proved
    );
    assert_eq!(
        LaneState::Agreed(TriState::Low).evidence_verdict(),
        EvidenceVerdict::Refuted
    );
    assert_eq!(
        LaneState::Agreed(TriState::HighZ).evidence_verdict(),
        EvidenceVerdict::Neither
    );

    let conflict = LaneState::Conflicted(LaneDispute {
        lane: "x".into(),
        drivers: Vec::new(),
    });
    assert_eq!(conflict.evidence_verdict(), EvidenceVerdict::Conflicted);
    assert!(conflict.is_conflicted());
}
