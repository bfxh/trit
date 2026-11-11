use crate::findings::{Finding, RuleOutcome, Severity};
use crate::rule::StructuralRule;
use crate::snapshot::{ColliderShape, ColliderSnapshot, EntityId, EntitySnapshot, WorldSnapshot};

mod math;
use self::math::*;

#[derive(Clone, Debug)]
pub struct PenetrationConfig {
    pub epsilon: f64,
    pub warning_depth: f64,
    pub critical_depth: f64,
    pub include_triggers: bool,
}

impl Default for PenetrationConfig {
    fn default() -> Self {
        Self {
            epsilon: 1.0e-4,
            warning_depth: 0.05,
            critical_depth: 0.5,
            include_triggers: false,
        }
    }
}

impl PenetrationConfig {
    fn severity(&self, depth: f64) -> Severity {
        if depth >= self.critical_depth {
            Severity::Critical
        } else if depth >= self.warning_depth {
            Severity::High
        } else {
            Severity::Medium
        }
    }
}

#[derive(Default)]
pub struct PenetrationOracle {
    pub config: PenetrationConfig,
}

impl StructuralRule for PenetrationOracle {
    fn id(&self) -> &'static str {
        "physics.penetration"
    }

    fn check(&self, snapshot: &WorldSnapshot) -> RuleOutcome {
        let mut prepared = Vec::new();
        let mut unknown = Vec::new();

        for collider in &snapshot.colliders {
            match prepare_collider(snapshot, collider, self.config.include_triggers) {
                Ok(Some(collider)) => prepared.push(collider),
                Ok(None) => {}
                Err(reason) => unknown.push(Finding::unknown(
                    self.id(),
                    Severity::Medium,
                    format!(
                        "penetration oracle could not evaluate collider on entity {}",
                        collider.entity
                    ),
                    reason,
                )),
            }
        }

        let mut failures = Vec::new();
        for i in 0..prepared.len() {
            for j in (i + 1)..prepared.len() {
                let a = &prepared[i];
                let b = &prepared[j];
                if !colliders_interact(a, b) {
                    continue;
                }
                match detect_pair(a, b) {
                    PairResult::Separated => {}
                    PairResult::Overlap(contact) => {
                        if contact.depth > self.config.epsilon.max(0.0) {
                            failures.push(self.penetration_finding(a, b, &contact));
                        }
                    }
                    PairResult::Unsupported(reason) => unknown.push(Finding::unknown(
                        self.id(),
                        Severity::Medium,
                        format!(
                            "penetration oracle skipped {} vs {} for entities {} and {}",
                            a.kind, b.kind, a.entity, b.entity
                        ),
                        reason,
                    )),
                }
            }
        }

        let has_failure = !failures.is_empty();
        let mut findings = unknown;
        findings.extend(failures);
        if has_failure {
            RuleOutcome::fail(self.id(), findings)
        } else if findings.is_empty() {
            RuleOutcome::pass(self.id())
        } else {
            RuleOutcome::unknown(self.id(), findings)
        }
    }
}

impl PenetrationOracle {
    fn penetration_finding(
        &self,
        a: &PreparedCollider,
        b: &PreparedCollider,
        contact: &Contact,
    ) -> Finding {
        Finding::fail(
            self.id(),
            self.config.severity(contact.depth),
            format!(
                "penetration between entity {} ({}) and entity {} ({})",
                a.entity, a.kind, b.entity, b.kind
            ),
            format!(
                "depth={:.6}; normal=({:.4},{:.4},{:.4}); point_a=({:.4},{:.4},{:.4}); point_b=({:.4},{:.4},{:.4}); layers={}/{}",
                contact.depth,
                contact.normal[0],
                contact.normal[1],
                contact.normal[2],
                contact.point_a[0],
                contact.point_a[1],
                contact.point_a[2],
                contact.point_b[0],
                contact.point_b[1],
                contact.point_b[2],
                a.layer,
                b.layer
            ),
        )
        .with_target(a.entity)
    }
}

#[derive(Clone, Debug)]
struct PreparedCollider {
    entity: EntityId,
    shape: PreparedShape,
    layer: u32,
    mask: u32,
    kind: &'static str,
}

#[derive(Clone, Debug)]
enum PreparedShape {
    Sphere(Sphere),
    Capsule(Capsule),
    Obb(Obb),
}

#[derive(Clone, Copy, Debug)]
struct Sphere {
    center: V3,
    radius: f64,
}

#[derive(Clone, Copy, Debug)]
struct Capsule {
    a: V3,
    b: V3,
    radius: f64,
}

#[derive(Clone, Copy, Debug)]
struct Obb {
    center: V3,
    axes: [V3; 3],
    half_extents: V3,
}

#[derive(Clone, Copy, Debug)]
struct Contact {
    depth: f64,
    normal: V3,
    point_a: V3,
    point_b: V3,
}

#[derive(Clone, Copy, Debug)]
enum PairResult {
    Separated,
    Overlap(Contact),
    Unsupported(&'static str),
}

fn prepare_collider(
    snapshot: &WorldSnapshot,
    collider: &ColliderSnapshot,
    include_triggers: bool,
) -> Result<Option<PreparedCollider>, String> {
    if !collider.enabled {
        return Ok(None);
    }
    if collider.is_trigger && !include_triggers {
        return Ok(None);
    }
    if collider.layer >= 32 {
        return Err(format!(
            "layer {} is outside the 32-bit collision mask",
            collider.layer
        ));
    }
    if !collider.shape.is_valid() {
        return Err(format!("invalid {} shape", collider.shape.kind_name()));
    }
    let entity = snapshot
        .find(collider.entity)
        .ok_or_else(|| format!("entity {} not found", collider.entity))?;
    prepare_shape(entity, collider).map(Some)
}

fn prepare_shape(
    entity: &EntitySnapshot,
    collider: &ColliderSnapshot,
) -> Result<PreparedCollider, String> {
    let transform = &entity.transform;
    if !transform.position.iter().all(|value| value.is_finite())
        || !transform.rotation.iter().all(|value| value.is_finite())
        || !transform.scale.iter().all(|value| value.is_finite())
    {
        return Err("transform contains non-finite values".into());
    }
    if !collider.local_offset.iter().all(|value| value.is_finite()) {
        return Err("local_offset contains non-finite values".into());
    }
    if transform.scale.iter().any(|value| value.abs() <= EPS) {
        return Err("transform scale is zero or too small".into());
    }

    let rotation =
        normalize_quat(transform.rotation).ok_or_else(|| "rotation is zero".to_string())?;
    let scale = [
        transform.scale[0].abs(),
        transform.scale[1].abs(),
        transform.scale[2].abs(),
    ];
    let center = add(
        transform.position,
        rotate(rotation, mul3(collider.local_offset, scale)),
    );

    let shape = match collider.shape {
        ColliderShape::Sphere { radius } => {
            if !is_uniform(scale) {
                return Err("non-uniform scale is unsupported for sphere colliders".into());
            }
            let world_radius = radius * scale[0];
            if world_radius <= EPS {
                return Err("sphere radius is too small after scaling".into());
            }
            PreparedShape::Sphere(Sphere {
                center,
                radius: world_radius,
            })
        }
        ColliderShape::Capsule {
            half_height,
            radius,
        } => {
            if !is_uniform(scale) {
                return Err("non-uniform scale is unsupported for capsule colliders".into());
            }
            let world_radius = radius * scale[0];
            let world_half_height = half_height * scale[0];
            if world_radius <= EPS {
                return Err("capsule radius is too small after scaling".into());
            }
            let axis = rotate(rotation, [0.0, 1.0, 0.0]);
            let offset = mul(axis, world_half_height);
            PreparedShape::Capsule(Capsule {
                a: sub(center, offset),
                b: add(center, offset),
                radius: world_radius,
            })
        }
        ColliderShape::Box { half_extents } => {
            let world_half_extents = [
                half_extents[0] * scale[0],
                half_extents[1] * scale[1],
                half_extents[2] * scale[2],
            ];
            if world_half_extents
                .iter()
                .any(|value| !value.is_finite() || *value <= EPS)
            {
                return Err("box half extents are too small after scaling".into());
            }
            PreparedShape::Obb(Obb {
                center,
                axes: [
                    rotate(rotation, [1.0, 0.0, 0.0]),
                    rotate(rotation, [0.0, 1.0, 0.0]),
                    rotate(rotation, [0.0, 0.0, 1.0]),
                ],
                half_extents: world_half_extents,
            })
        }
    };

    Ok(PreparedCollider {
        entity: entity.id,
        kind: collider.shape.kind_name(),
        shape,
        layer: collider.layer,
        mask: collider.mask,
    })
}

fn colliders_interact(a: &PreparedCollider, b: &PreparedCollider) -> bool {
    let a_hits_b = a.mask & (1u32 << b.layer) != 0;
    let b_hits_a = b.mask & (1u32 << a.layer) != 0;
    a_hits_b && b_hits_a
}

fn detect_pair(a: &PreparedCollider, b: &PreparedCollider) -> PairResult {
    match (&a.shape, &b.shape) {
        (PreparedShape::Sphere(a), PreparedShape::Sphere(b)) => detect_sphere_sphere(a, b),
        (PreparedShape::Sphere(a), PreparedShape::Capsule(b)) => detect_sphere_capsule(a, b),
        (PreparedShape::Capsule(a), PreparedShape::Sphere(b)) => {
            flip_result(detect_sphere_capsule(b, a))
        }
        (PreparedShape::Capsule(a), PreparedShape::Capsule(b)) => detect_capsule_capsule(a, b),
        (PreparedShape::Sphere(a), PreparedShape::Obb(b)) => detect_sphere_obb(a, b),
        (PreparedShape::Obb(a), PreparedShape::Sphere(b)) => flip_result(detect_sphere_obb(b, a)),
        (PreparedShape::Capsule(a), PreparedShape::Obb(b)) => detect_capsule_obb(a, b),
        (PreparedShape::Obb(a), PreparedShape::Capsule(b)) => flip_result(detect_capsule_obb(b, a)),
        (PreparedShape::Obb(a), PreparedShape::Obb(b)) => detect_obb_obb(a, b),
    }
}

fn flip_result(result: PairResult) -> PairResult {
    match result {
        PairResult::Separated => PairResult::Separated,
        PairResult::Unsupported(reason) => PairResult::Unsupported(reason),
        PairResult::Overlap(contact) => PairResult::Overlap(Contact {
            depth: contact.depth,
            normal: mul(contact.normal, -1.0),
            point_a: contact.point_b,
            point_b: contact.point_a,
        }),
    }
}

fn detect_sphere_sphere(a: &Sphere, b: &Sphere) -> PairResult {
    let delta = sub(b.center, a.center);
    let distance = length(delta);
    let depth = a.radius + b.radius - distance;
    if depth <= 0.0 {
        return PairResult::Separated;
    }
    let normal = if distance > EPS {
        scale(delta, 1.0 / distance)
    } else {
        [1.0, 0.0, 0.0]
    };
    PairResult::Overlap(Contact {
        depth,
        normal,
        point_a: add(a.center, mul(normal, a.radius)),
        point_b: sub(b.center, mul(normal, b.radius)),
    })
}

fn detect_sphere_capsule(sphere: &Sphere, capsule: &Capsule) -> PairResult {
    let closest = closest_on_segment(sphere.center, capsule.a, capsule.b);
    let delta = sub(closest, sphere.center);
    let distance = length(delta);
    let depth = sphere.radius + capsule.radius - distance;
    if depth <= 0.0 {
        return PairResult::Separated;
    }
    let normal = if distance > EPS {
        scale(delta, 1.0 / distance)
    } else {
        [1.0, 0.0, 0.0]
    };
    PairResult::Overlap(Contact {
        depth,
        normal,
        point_a: add(sphere.center, mul(normal, sphere.radius)),
        point_b: sub(closest, mul(normal, capsule.radius)),
    })
}

fn detect_capsule_capsule(a: &Capsule, b: &Capsule) -> PairResult {
    let (point_a, point_b) = closest_segment_segment(a.a, a.b, b.a, b.b);
    let delta = sub(point_b, point_a);
    let distance = length(delta);
    let depth = a.radius + b.radius - distance;
    if depth <= 0.0 {
        return PairResult::Separated;
    }
    let normal = if distance > EPS {
        scale(delta, 1.0 / distance)
    } else {
        [1.0, 0.0, 0.0]
    };
    PairResult::Overlap(Contact {
        depth,
        normal,
        point_a: add(point_a, mul(normal, a.radius)),
        point_b: sub(point_b, mul(normal, b.radius)),
    })
}

fn detect_sphere_obb(sphere: &Sphere, obb: &Obb) -> PairResult {
    let local_center = obb_to_local(obb, sphere.center);
    let mut local_closest = [0.0; 3];
    for (axis, value) in local_center.iter().enumerate() {
        local_closest[axis] = value.clamp(-obb.half_extents[axis], obb.half_extents[axis]);
    }
    let delta = sub(local_center, local_closest);
    let distance = length(delta);

    if distance > EPS {
        let depth = sphere.radius - distance;
        if depth <= 0.0 {
            return PairResult::Separated;
        }
        let local_normal = scale(delta, -1.0 / distance);
        let normal = obb_dir_to_world(obb, local_normal);
        return PairResult::Overlap(Contact {
            depth,
            normal,
            point_a: add(sphere.center, mul(normal, sphere.radius)),
            point_b: obb_point_to_world(obb, local_closest),
        });
    }

    let mut face_axis = 0;
    let mut face_distance = f64::INFINITY;
    for (axis, value) in local_center.iter().enumerate() {
        let distance_to_face = obb.half_extents[axis] - value.abs();
        if distance_to_face < face_distance {
            face_distance = distance_to_face;
            face_axis = axis;
        }
    }
    let sign = if local_center[face_axis] >= 0.0 {
        1.0
    } else {
        -1.0
    };
    let mut local_normal = [0.0; 3];
    local_normal[face_axis] = sign;
    let mut local_face = local_center;
    local_face[face_axis] = sign * obb.half_extents[face_axis];
    let normal = obb_dir_to_world(obb, local_normal);
    PairResult::Overlap(Contact {
        depth: sphere.radius + face_distance,
        normal,
        point_a: add(sphere.center, mul(normal, sphere.radius)),
        point_b: obb_point_to_world(obb, local_face),
    })
}

fn detect_obb_obb(a: &Obb, b: &Obb) -> PairResult {
    let center_delta = sub(b.center, a.center);
    let mut axes = Vec::with_capacity(15);
    axes.extend(a.axes);
    axes.extend(b.axes);
    for i in 0..3 {
        for j in 0..3 {
            let axis = cross(a.axes[i], b.axes[j]);
            if dot(axis, axis) > EPS * EPS {
                axes.push(axis);
            }
        }
    }

    let mut best_depth = f64::INFINITY;
    let mut best_normal = [1.0, 0.0, 0.0];

    for axis in axes {
        let axis_length = length(axis);
        if axis_length <= EPS {
            continue;
        }
        let normal = scale(axis, 1.0 / axis_length);
        let radius_a = projection_radius(a, normal);
        let radius_b = projection_radius(b, normal);
        let distance = dot(center_delta, normal).abs();
        let overlap = radius_a + radius_b - distance;
        if overlap <= 0.0 {
            return PairResult::Separated;
        }
        if overlap < best_depth {
            best_depth = overlap;
            best_normal = if dot(center_delta, normal) >= 0.0 {
                normal
            } else {
                mul(normal, -1.0)
            };
        }
    }

    if !best_depth.is_finite() {
        return PairResult::Unsupported("no valid OBB separating axis");
    }

    let point_a = obb_support(a, best_normal);
    let point_b = obb_support(b, mul(best_normal, -1.0));
    PairResult::Overlap(Contact {
        depth: best_depth,
        normal: best_normal,
        point_a,
        point_b,
    })
}

fn detect_capsule_obb(capsule: &Capsule, obb: &Obb) -> PairResult {
    let p0 = obb_to_local(obb, capsule.a);
    let p1 = obb_to_local(obb, capsule.b);

    if let Some((t_min, t_max)) = segment_aabb_interval(p0, p1, obb.half_extents) {
        let t = (t_min + t_max) * 0.5;
        let point = lerp(p0, p1, t);
        let mut face_axis = 0;
        let mut face_distance = f64::INFINITY;
        for (axis, value) in point.iter().enumerate() {
            let distance_to_face = obb.half_extents[axis] - value.abs();
            if distance_to_face < face_distance {
                face_distance = distance_to_face;
                face_axis = axis;
            }
        }
        let sign = if point[face_axis] >= 0.0 { 1.0 } else { -1.0 };
        let mut local_normal = [0.0; 3];
        local_normal[face_axis] = sign;
        let mut local_face = point;
        local_face[face_axis] = sign * obb.half_extents[face_axis];
        let normal = obb_dir_to_world(obb, local_normal);
        return PairResult::Overlap(Contact {
            depth: capsule.radius + face_distance,
            normal,
            point_a: add(obb_point_to_world(obb, point), mul(normal, capsule.radius)),
            point_b: obb_point_to_world(obb, local_face),
        });
    }

    let (point, closest) = closest_segment_aabb(p0, p1, obb.half_extents);
    let delta = sub(closest, point);
    let distance = length(delta);
    let depth = capsule.radius - distance;
    if depth <= 0.0 {
        return PairResult::Separated;
    }
    let local_normal = if distance > EPS {
        scale(delta, 1.0 / distance)
    } else {
        [1.0, 0.0, 0.0]
    };
    let normal = obb_dir_to_world(obb, local_normal);
    PairResult::Overlap(Contact {
        depth,
        normal,
        point_a: add(obb_point_to_world(obb, point), mul(normal, capsule.radius)),
        point_b: obb_point_to_world(obb, closest),
    })
}

fn projection_radius(obb: &Obb, axis: V3) -> f64 {
    obb.half_extents[0] * dot(axis, obb.axes[0]).abs()
        + obb.half_extents[1] * dot(axis, obb.axes[1]).abs()
        + obb.half_extents[2] * dot(axis, obb.axes[2]).abs()
}

fn obb_support(obb: &Obb, direction: V3) -> V3 {
    let mut point = obb.center;
    for axis in 0..3 {
        let sign = if dot(direction, obb.axes[axis]) >= 0.0 {
            1.0
        } else {
            -1.0
        };
        point = add(point, mul(obb.axes[axis], obb.half_extents[axis] * sign));
    }
    point
}

fn obb_to_local(obb: &Obb, point: V3) -> V3 {
    let delta = sub(point, obb.center);
    [
        dot(delta, obb.axes[0]),
        dot(delta, obb.axes[1]),
        dot(delta, obb.axes[2]),
    ]
}

fn obb_dir_to_world(obb: &Obb, local: V3) -> V3 {
    add(
        add(mul(obb.axes[0], local[0]), mul(obb.axes[1], local[1])),
        mul(obb.axes[2], local[2]),
    )
}

fn obb_point_to_world(obb: &Obb, local: V3) -> V3 {
    add(obb.center, obb_dir_to_world(obb, local))
}
