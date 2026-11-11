//! 纯几何/数值辅助：向量、四元数、线段与 AABB 最近点。

pub(super) type V3 = [f64; 3];

pub(super) const EPS: f64 = 1.0e-9;

pub(super) fn segment_aabb_interval(p0: V3, p1: V3, half_extents: V3) -> Option<(f64, f64)> {
    let direction = sub(p1, p0);
    if dot(direction, direction) <= EPS * EPS {
        let inside = (0..3).all(|axis| p0[axis].abs() <= half_extents[axis]);
        return if inside { Some((0.0, 0.0)) } else { None };
    }

    let mut t_min = 0.0_f64;
    let mut t_max = 1.0_f64;
    for axis in 0..3 {
        if direction[axis].abs() <= EPS {
            if p0[axis].abs() > half_extents[axis] {
                return None;
            }
            continue;
        }
        let mut t1 = (-half_extents[axis] - p0[axis]) / direction[axis];
        let mut t2 = (half_extents[axis] - p0[axis]) / direction[axis];
        if t1 > t2 {
            std::mem::swap(&mut t1, &mut t2);
        }
        t_min = t_min.max(t1);
        t_max = t_max.min(t2);
        if t_min > t_max {
            return None;
        }
    }
    Some((t_min, t_max))
}

pub(super) fn closest_segment_aabb(p0: V3, p1: V3, half_extents: V3) -> (V3, V3) {
    let mut low = 0.0;
    let mut high = 1.0;
    for _ in 0..80 {
        let third = (high - low) / 3.0;
        let t1 = low + third;
        let t2 = high - third;
        let f1 = aabb_distance_squared(lerp(p0, p1, t1), half_extents);
        let f2 = aabb_distance_squared(lerp(p0, p1, t2), half_extents);
        if f1 < f2 {
            high = t2;
        } else {
            low = t1;
        }
    }
    let t = (low + high) * 0.5;
    let point = lerp(p0, p1, t);
    (point, clamp_to_aabb(point, half_extents))
}

pub(super) fn aabb_distance_squared(point: V3, half_extents: V3) -> f64 {
    let closest = clamp_to_aabb(point, half_extents);
    let delta = sub(point, closest);
    dot(delta, delta)
}

pub(super) fn clamp_to_aabb(point: V3, half_extents: V3) -> V3 {
    [
        point[0].clamp(-half_extents[0], half_extents[0]),
        point[1].clamp(-half_extents[1], half_extents[1]),
        point[2].clamp(-half_extents[2], half_extents[2]),
    ]
}

pub(super) fn closest_on_segment(point: V3, a: V3, b: V3) -> V3 {
    let direction = sub(b, a);
    let denom = dot(direction, direction);
    if denom <= EPS {
        return a;
    }
    let t = dot(sub(point, a), direction) / denom;
    add(a, mul(direction, t.clamp(0.0, 1.0)))
}

pub(super) fn closest_segment_segment(p0: V3, p1: V3, q0: V3, q1: V3) -> (V3, V3) {
    let d1 = sub(p1, p0);
    let d2 = sub(q1, q0);
    let r = sub(p0, q0);
    let a = dot(d1, d1);
    let e = dot(d2, d2);
    let f = dot(d2, r);

    let (mut s, mut t);
    if a <= EPS && e <= EPS {
        s = 0.0;
        t = 0.0;
    } else if a <= EPS {
        s = 0.0;
        t = (f / e).clamp(0.0, 1.0);
    } else {
        let c = dot(d1, r);
        if e <= EPS {
            t = 0.0;
            s = (-c / a).clamp(0.0, 1.0);
        } else {
            let b = dot(d1, d2);
            let denom = a * e - b * b;
            if denom > EPS {
                s = ((b * f - c * e) / denom).clamp(0.0, 1.0);
            } else {
                s = 0.0;
            }
            t = (b * s + f) / e;
            if t < 0.0 {
                t = 0.0;
                s = (-c / a).clamp(0.0, 1.0);
            } else if t > 1.0 {
                t = 1.0;
                s = ((b - c) / a).clamp(0.0, 1.0);
            }
        }
    }

    (add(p0, mul(d1, s)), add(q0, mul(d2, t)))
}

pub(super) fn is_uniform(scale: V3) -> bool {
    let reference = scale[0];
    scale
        .iter()
        .all(|value| (value - reference).abs() <= 1.0e-6 * reference.max(1.0))
}

pub(super) fn normalize_quat(quaternion: [f64; 4]) -> Option<[f64; 4]> {
    let norm = quaternion
        .iter()
        .map(|value| value * value)
        .sum::<f64>()
        .sqrt();
    if !norm.is_finite() || norm <= EPS {
        return None;
    }
    Some([
        quaternion[0] / norm,
        quaternion[1] / norm,
        quaternion[2] / norm,
        quaternion[3] / norm,
    ])
}

pub(super) fn rotate(quaternion: [f64; 4], vector: V3) -> V3 {
    let q = [quaternion[0], quaternion[1], quaternion[2]];
    let w = quaternion[3];
    let t = mul(cross(q, vector), 2.0);
    add(add(vector, mul(t, w)), cross(q, t))
}

pub(super) fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub(super) fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub(super) fn mul(vector: V3, scalar: f64) -> V3 {
    [vector[0] * scalar, vector[1] * scalar, vector[2] * scalar]
}

pub(super) fn mul3(a: V3, b: V3) -> V3 {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2]]
}

pub(super) fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(super) fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub(super) fn length(vector: V3) -> f64 {
    dot(vector, vector).sqrt()
}

pub(super) fn scale(vector: V3, scalar: f64) -> V3 {
    mul(vector, scalar)
}

pub(super) fn lerp(a: V3, b: V3, t: f64) -> V3 {
    add(a, mul(sub(b, a), t))
}
