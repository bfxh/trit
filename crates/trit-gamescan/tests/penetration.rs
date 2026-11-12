use trit_gamescan::{
    ColliderShape, ColliderSnapshot, EntityId, EntitySnapshot, PenetrationOracle, RuleOutcome,
    StructuralRule, Ternary, WorldSnapshot,
};

fn snapshot_with(shapes: &[(EntityId, ColliderShape, [f64; 3])]) -> WorldSnapshot {
    let mut snapshot = WorldSnapshot::new("test");
    for (id, shape, position) in shapes {
        let mut entity = EntitySnapshot::new(*id);
        entity.transform.position = *position;
        snapshot.entities.push(entity);
        snapshot
            .colliders
            .push(ColliderSnapshot::new(*id, shape.clone()));
    }
    snapshot
}

fn check(snapshot: &WorldSnapshot) -> RuleOutcome {
    PenetrationOracle::default().check(snapshot)
}

#[test]
fn spheres_overlap_and_separate() {
    let overlapping = snapshot_with(&[
        (1, ColliderShape::Sphere { radius: 0.5 }, [0.0, 0.0, 0.0]),
        (2, ColliderShape::Sphere { radius: 0.5 }, [0.5, 0.0, 0.0]),
    ]);
    assert_eq!(check(&overlapping).trit, Ternary::False);

    let separated = snapshot_with(&[
        (1, ColliderShape::Sphere { radius: 0.5 }, [0.0, 0.0, 0.0]),
        (2, ColliderShape::Sphere { radius: 0.5 }, [2.0, 0.0, 0.0]),
    ]);
    assert_eq!(check(&separated).trit, Ternary::True);
}

#[test]
fn box_box_overlap_is_detected() {
    let snapshot = snapshot_with(&[
        (
            1,
            ColliderShape::Box {
                half_extents: [1.0, 1.0, 1.0],
            },
            [0.0, 0.0, 0.0],
        ),
        (
            2,
            ColliderShape::Box {
                half_extents: [1.0, 1.0, 1.0],
            },
            [1.5, 0.0, 0.0],
        ),
    ]);
    let outcome = check(&snapshot);
    assert_eq!(outcome.trit, Ternary::False);
    assert!(
        outcome
            .findings
            .iter()
            .any(|finding| { finding.message.contains("penetration") })
    );
}

#[test]
fn capsule_capsule_overlap_is_detected() {
    let snapshot = snapshot_with(&[
        (
            1,
            ColliderShape::Capsule {
                half_height: 1.0,
                radius: 0.2,
            },
            [0.0, 0.0, 0.0],
        ),
        (
            2,
            ColliderShape::Capsule {
                half_height: 1.0,
                radius: 0.2,
            },
            [0.3, 0.0, 0.0],
        ),
    ]);
    assert_eq!(check(&snapshot).trit, Ternary::False);
}

#[test]
fn sphere_box_overlap_is_detected() {
    let snapshot = snapshot_with(&[
        (1, ColliderShape::Sphere { radius: 1.0 }, [0.0, 0.0, 0.0]),
        (
            2,
            ColliderShape::Box {
                half_extents: [0.5, 0.5, 0.5],
            },
            [0.8, 0.0, 0.0],
        ),
    ]);
    assert_eq!(check(&snapshot).trit, Ternary::False);
}

#[test]
fn capsule_box_overlap_is_detected() {
    let snapshot = snapshot_with(&[
        (
            1,
            ColliderShape::Capsule {
                half_height: 1.0,
                radius: 0.3,
            },
            [0.0, 0.0, 0.0],
        ),
        (
            2,
            ColliderShape::Box {
                half_extents: [0.5, 0.5, 0.5],
            },
            [0.7, 0.0, 0.0],
        ),
    ]);
    assert_eq!(check(&snapshot).trit, Ternary::False);
}

#[test]
fn trigger_colliders_are_ignored_by_default() {
    let mut snapshot = snapshot_with(&[
        (1, ColliderShape::Sphere { radius: 0.5 }, [0.0, 0.0, 0.0]),
        (2, ColliderShape::Sphere { radius: 0.5 }, [0.2, 0.0, 0.0]),
    ]);
    snapshot.colliders[1].is_trigger = true;
    assert_eq!(check(&snapshot).trit, Ternary::True);
}

#[test]
fn layer_mask_can_disable_pairs() {
    let mut snapshot = snapshot_with(&[
        (1, ColliderShape::Sphere { radius: 0.5 }, [0.0, 0.0, 0.0]),
        (2, ColliderShape::Sphere { radius: 0.5 }, [0.2, 0.0, 0.0]),
    ]);
    snapshot.colliders[0].mask = 0;
    assert_eq!(check(&snapshot).trit, Ternary::True);
}

#[test]
fn non_uniform_sphere_scale_is_unknown() {
    let mut snapshot = snapshot_with(&[
        (1, ColliderShape::Sphere { radius: 0.5 }, [0.0, 0.0, 0.0]),
        (2, ColliderShape::Sphere { radius: 0.5 }, [2.0, 0.0, 0.0]),
    ]);
    snapshot.entities[0].transform.scale = [1.0, 2.0, 1.0];
    assert_eq!(check(&snapshot).trit, Ternary::Unknown);
}

#[test]
fn rotated_boxes_are_detected() {
    let angle = std::f64::consts::FRAC_PI_4;
    let rotation = [0.0, 0.0, (angle * 0.5).sin(), (angle * 0.5).cos()];
    let mut snapshot = snapshot_with(&[
        (
            1,
            ColliderShape::Box {
                half_extents: [1.0, 1.0, 1.0],
            },
            [0.0, 0.0, 0.0],
        ),
        (
            2,
            ColliderShape::Box {
                half_extents: [1.0, 1.0, 1.0],
            },
            [1.4, 0.0, 0.0],
        ),
    ]);
    snapshot.entities[0].transform.rotation = rotation;
    assert_eq!(check(&snapshot).trit, Ternary::False);
}
