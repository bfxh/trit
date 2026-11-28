use super::*;

fn spec(class: ActionClass, tainted: bool) -> ActionSpec {
    ActionSpec {
        class,
        target: None,
        from_untrusted_data: tainted,
    }
}

fn spec_at(class: ActionClass, target: &str, tainted: bool) -> ActionSpec {
    ActionSpec {
        class,
        target: Some(target.to_owned()),
        from_untrusted_data: tainted,
    }
}

fn policy(classes: &[ActionClass], allow_untrusted_origin: bool) -> CapabilityPolicy {
    CapabilityPolicy {
        allow: classes.iter().copied().collect(),
        allow_untrusted_origin,
        allow_targets: BTreeMap::new(),
    }
}

fn targeted_policy(
    class: ActionClass,
    targets: &[&str],
    allow_untrusted_origin: bool,
) -> CapabilityPolicy {
    let mut allow_targets = BTreeMap::new();
    allow_targets.insert(class, targets.iter().map(|s| s.to_string()).collect());
    CapabilityPolicy {
        allow: [class].into_iter().collect(),
        allow_untrusted_origin,
        allow_targets,
    }
}

#[test]
fn read_is_allowed_even_from_untrusted_data() {
    let decision = decide(&policy(&[], false), &spec(ActionClass::Read, true));
    assert_eq!(decision.verdict, Ternary::True);
    assert_eq!(decision.reason, CapabilityReason::ReadOnly);
}

#[test]
fn side_effecting_classes_are_denied_by_default() {
    for class in [ActionClass::Write, ActionClass::Exec, ActionClass::Network] {
        let decision = decide(&CapabilityPolicy::read_only(), &spec(class, false));
        assert_eq!(decision.verdict, Ternary::False, "{class:?}");
        assert_eq!(decision.reason, CapabilityReason::ClassNotGranted);
    }
}

#[test]
fn granted_side_effect_is_true_when_trusted() {
    let decision = decide(
        &policy(&[ActionClass::Write], false),
        &spec(ActionClass::Write, false),
    );
    assert_eq!(decision.verdict, Ternary::True);
    assert_eq!(decision.reason, CapabilityReason::Granted);
}

#[test]
fn untrusted_origin_turns_grant_into_confirmation() {
    let decision = decide(
        &policy(&[ActionClass::Network], false),
        &spec(ActionClass::Network, true),
    );
    assert_eq!(decision.verdict, Ternary::Unknown);
    assert_eq!(
        decision.reason,
        CapabilityReason::UntrustedOriginRequiresConfirmation
    );
}

#[test]
fn explicit_untrusted_grant_is_true_and_reported() {
    let decision = decide(
        &policy(&[ActionClass::Exec], true),
        &spec(ActionClass::Exec, true),
    );
    assert_eq!(decision.verdict, Ternary::True);
    assert_eq!(decision.reason, CapabilityReason::GrantedForUntrustedOrigin);
}

#[test]
fn target_whitelist_requires_a_target() {
    let policy = targeted_policy(ActionClass::Write, &["docs/out.md"], false);
    let decision = decide(&policy, &spec(ActionClass::Write, false));
    assert_eq!(decision.verdict, Ternary::False);
    assert_eq!(decision.reason, CapabilityReason::TargetRequired);
    assert_eq!(decision.target, None);
}

#[test]
fn target_outside_whitelist_is_refused() {
    let policy = targeted_policy(ActionClass::Write, &["docs/out.md"], false);
    let decision = decide(
        &policy,
        &spec_at(ActionClass::Write, "../etc/passwd", false),
    );
    assert_eq!(decision.verdict, Ternary::False);
    assert_eq!(decision.reason, CapabilityReason::TargetNotAllowed);
    assert_eq!(decision.target.as_deref(), Some("../etc/passwd"));
}

#[test]
fn allowed_target_still_goes_through_origin_gate() {
    let policy = targeted_policy(ActionClass::Network, &["api.example.com"], false);
    assert_eq!(
        decide(
            &policy,
            &spec_at(ActionClass::Network, "api.example.com", false)
        )
        .verdict,
        Ternary::True
    );
    let tainted = decide(
        &policy,
        &spec_at(ActionClass::Network, "api.example.com", true),
    );
    assert_eq!(tainted.verdict, Ternary::Unknown);
    assert_eq!(
        tainted.reason,
        CapabilityReason::UntrustedOriginRequiresConfirmation
    );
}

#[test]
fn target_listing_without_class_grant_is_still_denied() {
    let mut allow_targets = BTreeMap::new();
    allow_targets.insert(
        ActionClass::Write,
        ["docs/out.md".to_string()].into_iter().collect(),
    );
    let policy = CapabilityPolicy {
        allow: [ActionClass::Read].into_iter().collect(),
        allow_untrusted_origin: false,
        allow_targets,
    };
    let decision = decide(&policy, &spec_at(ActionClass::Write, "docs/out.md", false));
    assert_eq!(decision.verdict, Ternary::False);
    assert_eq!(decision.reason, CapabilityReason::ClassNotGranted);
}

#[test]
fn read_ignores_target_whitelist() {
    let mut policy = targeted_policy(ActionClass::Read, &["docs/out.md"], false);
    policy.allow.insert(ActionClass::Read);
    let decision = decide(&policy, &spec_at(ActionClass::Read, "elsewhere", true));
    assert_eq!(decision.verdict, Ternary::True);
    assert_eq!(decision.reason, CapabilityReason::ReadOnly);
}

#[test]
fn gate_matches_decide() {
    let policy = policy(&[ActionClass::Write], false);
    assert_eq!(
        capability_gate(&policy, &spec(ActionClass::Write, true)),
        Ternary::Unknown
    );
    assert_eq!(
        capability_gate(&policy, &spec(ActionClass::Network, false)),
        Ternary::False
    );
}

#[test]
fn policy_parses_from_json_and_refuses_unknown_class() {
    let parsed: CapabilityPolicy = serde_json::from_str(
        r#"{"allow":["read","write"],"allow_untrusted_origin":false,
            "allow_targets":{"write":["docs/out.md"]}}"#,
    )
    .expect("policy");
    assert!(parsed.allow.contains(&ActionClass::Write));
    assert!(!parsed.allow_untrusted_origin);
    assert!(
        parsed.allow_targets[&ActionClass::Write].contains("docs/out.md"),
        "目标白名单要能解析"
    );

    let bad = serde_json::from_str::<ActionSpec>(r#"{"class":"shell"}"#);
    assert!(bad.is_err(), "未知动作类别必须在解析层拒绝");
}
