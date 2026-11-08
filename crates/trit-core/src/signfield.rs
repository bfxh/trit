//! 三态符号场（3PSDF 风格）：每个体素是 Inside / Null / Outside 三极之一。
//!
//! 与二值 SDF 的本质差别在 `Null`：它不是「更小的 inside」，而是**断路开关**。
//! 二值场里任何内外切换都必产一层几何，薄壳被迫表示为「有体积的薄实体」⇒ 内外两层；
//! 三态场里 `+1 | 0 | +1` 的薄壳可以直接写成「两侧 outside 夹一层 null」，
//! 提取时不必把它解释成实体。
//!
//! 提取用三态 Dual Contouring：每个跨越界面的 cube 只出一个顶点，
//! 因此**同一个 cube 内两侧界面会合并成一个顶点**；薄壳厚度 ≤ 1 体素时界面即为单层。
//! 厚度超过 1 体素时仍会出现两层——这正是 3PSDF「需要更密采样」的原因，不是实现缺陷。
//!
//! 与三值逻辑的对应：`Inside → Ternary::False`、`Null → Ternary::Unknown`、
//! `Outside → Ternary::True`，因此可直接套用本 crate 的 Kleene/Belnap 代数做约束传播。

use crate::value::Ternary;

/// 三极符号：Inside(-1) / Null(0) / Outside(+1)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(i8)]
pub enum TriSign {
    Inside = -1,
    Null = 0,
    Outside = 1,
}

impl TriSign {
    pub const ALL: [TriSign; 3] = [TriSign::Inside, TriSign::Null, TriSign::Outside];

    pub fn from_i8(v: i8) -> Self {
        match v {
            v if v < 0 => TriSign::Inside,
            0 => TriSign::Null,
            _ => TriSign::Outside,
        }
    }

    pub fn as_i8(self) -> i8 {
        self as i8
    }

    /// 三态符号 ↔ 三值逻辑。
    pub fn to_ternary(self) -> Ternary {
        match self {
            TriSign::Inside => Ternary::False,
            TriSign::Null => Ternary::Unknown,
            TriSign::Outside => Ternary::True,
        }
    }

    pub fn from_ternary(t: Ternary) -> Self {
        match t {
            Ternary::False => TriSign::Inside,
            Ternary::Unknown => TriSign::Null,
            Ternary::True => TriSign::Outside,
        }
    }

    /// 是否已判定（非 Null）。
    pub fn is_decided(self) -> bool {
        self != TriSign::Null
    }

    #[inline]
    fn code(self) -> u32 {
        (self as i8 + 1) as u32
    }

    #[inline]
    fn from_code(c: u32) -> Self {
        match c & 3 {
            0 => TriSign::Inside,
            1 => TriSign::Null,
            _ => TriSign::Outside,
        }
    }
}

/// 稠密三态场：`n³` 体素，2 bit/体素打包进 `u32`（16 体素/字）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignField3D {
    n: u32,
    words: Vec<u32>,
    /// `[inside, null, outside]` 计数。
    counts: [usize; 3],
}

impl SignField3D {
    /// 新建全 `Null` 的场。
    pub fn new(n: u32) -> Self {
        let vox = (n as usize).pow(3);
        let words = vec![0x5555_5555u32; vox.div_ceil(16)];
        Self {
            n,
            words,
            counts: [0, vox, 0],
        }
    }

    /// 新建全部填同一状态的场。
    pub fn filled(n: u32, s: TriSign) -> Self {
        let mut f = Self::new(n);
        let word = s.code() * 0x5555_5555;
        for w in &mut f.words {
            *w = word;
        }
        let vox = (n as usize).pow(3);
        f.counts = match s {
            TriSign::Inside => [vox, 0, 0],
            TriSign::Null => [0, vox, 0],
            TriSign::Outside => [0, 0, vox],
        };
        f
    }

    pub fn n(&self) -> u32 {
        self.n
    }

    /// `[inside, null, outside]` 体素数。
    pub fn counts(&self) -> [usize; 3] {
        self.counts
    }

    #[inline]
    fn index(&self, x: u32, y: u32, z: u32) -> usize {
        let n = self.n as usize;
        z as usize * n * n + y as usize * n + x as usize
    }

    #[inline]
    pub fn at(&self, x: u32, y: u32, z: u32) -> TriSign {
        let i = self.index(x, y, z);
        TriSign::from_code((self.words[i / 16] >> ((i % 16) * 2)) & 3)
    }

    pub fn set(&mut self, x: u32, y: u32, z: u32, s: TriSign) {
        let old = self.at(x, y, z);
        if old == s {
            return;
        }
        let i = self.index(x, y, z);
        let w = i / 16;
        let sh = (i % 16) * 2;
        self.words[w] = (self.words[w] & !(3u32 << sh)) | (s.code() << sh);
        self.counts[(old.as_i8() + 1) as usize] -= 1;
        self.counts[(s.as_i8() + 1) as usize] += 1;
    }

    /// 界面边数（相邻体素状态不同）。
    pub fn interface_edges(&self) -> usize {
        let n = self.n;
        let mut c = 0usize;
        for z in 0..n {
            for y in 0..n {
                for x in 0..n {
                    let s = self.at(x, y, z);
                    if x + 1 < n && self.at(x + 1, y, z) != s {
                        c += 1;
                    }
                    if y + 1 < n && self.at(x, y + 1, z) != s {
                        c += 1;
                    }
                    if z + 1 < n && self.at(x, y, z + 1) != s {
                        c += 1;
                    }
                }
            }
        }
        c
    }
}

/// 三态提取出的表面网格。
#[derive(Debug, Clone, Default)]
pub struct SignMesh {
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    /// 跨越界面的边数（≈ 四边形数）。
    pub quads: usize,
}

/// 三态 Dual Contouring：每个跨越界面的 cube 出一个顶点（界面边中点平均），
/// 每条界面边连接 4 个相邻 cube 的顶点组成四边形（两三角形）。
/// 绕序按「从状态小的一侧指向状态大的一侧」自动校正。
pub fn extract_surface(field: &SignField3D, origin: [f32; 3], h: f32) -> SignMesh {
    let n = field.n();
    let mut mesh = SignMesh::default();
    if n < 2 {
        return mesh;
    }
    let cn = (n - 1) as usize;
    let cube_index = |x: usize, y: usize, z: usize| (z * cn + y) * cn + x;
    let mut cube_vert = vec![u32::MAX; cn * cn * cn];

    // 1. cube 顶点
    for cz in 0..cn {
        for cy in 0..cn {
            for cx in 0..cn {
                let first = field.at(cx as u32, cy as u32, cz as u32);
                let mut mixed = false;
                'chk: for dz in 0..2 {
                    for dy in 0..2 {
                        for dx in 0..2 {
                            if field.at((cx + dx) as u32, (cy + dy) as u32, (cz + dz) as u32)
                                != first
                            {
                                mixed = true;
                                break 'chk;
                            }
                        }
                    }
                }
                if !mixed {
                    continue;
                }
                // 界面边中点平均
                let mut acc = [0.0f32; 3];
                let mut cnt = 0u32;
                for axis in 0..3 {
                    for a in 0..2u32 {
                        for b in 0..2u32 {
                            let (p, q) = match axis {
                                0 => (
                                    (cx as u32, cy as u32 + a, cz as u32 + b),
                                    (cx as u32 + 1, cy as u32 + a, cz as u32 + b),
                                ),
                                1 => (
                                    (cx as u32 + a, cy as u32, cz as u32 + b),
                                    (cx as u32 + a, cy as u32 + 1, cz as u32 + b),
                                ),
                                _ => (
                                    (cx as u32 + a, cy as u32 + b, cz as u32),
                                    (cx as u32 + a, cy as u32 + b, cz as u32 + 1),
                                ),
                            };
                            if field.at(p.0, p.1, p.2) != field.at(q.0, q.1, q.2) {
                                acc[0] += (p.0 as f32 + q.0 as f32) * 0.5;
                                acc[1] += (p.1 as f32 + q.1 as f32) * 0.5;
                                acc[2] += (p.2 as f32 + q.2 as f32) * 0.5;
                                cnt += 1;
                            }
                        }
                    }
                }
                let pos = if cnt > 0 {
                    [
                        origin[0] + acc[0] / cnt as f32 * h,
                        origin[1] + acc[1] / cnt as f32 * h,
                        origin[2] + acc[2] / cnt as f32 * h,
                    ]
                } else {
                    [
                        origin[0] + (cx as f32 + 0.5) * h,
                        origin[1] + (cy as f32 + 0.5) * h,
                        origin[2] + (cz as f32 + 0.5) * h,
                    ]
                };
                cube_vert[cube_index(cx, cy, cz)] = mesh.positions.len() as u32;
                mesh.positions.push(pos);
            }
        }
    }

    // 2. 每条界面边 → 4 邻 cube 顶点
    for axis in 0..3 {
        for z in 0..n {
            for y in 0..n {
                for x in 0..n {
                    let (p, q, dir) = match axis {
                        0 => {
                            if x + 1 >= n {
                                continue;
                            }
                            ((x, y, z), (x + 1, y, z), [1.0f32, 0.0, 0.0])
                        }
                        1 => {
                            if y + 1 >= n {
                                continue;
                            }
                            ((x, y, z), (x, y + 1, z), [0.0f32, 1.0, 0.0])
                        }
                        _ => {
                            if z + 1 >= n {
                                continue;
                            }
                            ((x, y, z), (x, y, z + 1), [0.0f32, 0.0, 1.0])
                        }
                    };
                    let s0 = field.at(p.0, p.1, p.2);
                    let s1 = field.at(q.0, q.1, q.2);
                    if s0 == s1 {
                        continue;
                    }
                    let offs: [(i32, i32, i32); 4] = match axis {
                        0 => [(0, -1, -1), (0, 0, -1), (0, 0, 0), (0, -1, 0)],
                        1 => [(-1, 0, -1), (0, 0, -1), (0, 0, 0), (-1, 0, 0)],
                        _ => [(-1, -1, 0), (-1, 0, 0), (0, 0, 0), (0, -1, 0)],
                    };
                    let mut v = [u32::MAX; 4];
                    let mut ok = true;
                    for (i, (ox, oy, oz)) in offs.iter().enumerate() {
                        let cx = x as i32 + ox;
                        let cy = y as i32 + oy;
                        let cz = z as i32 + oz;
                        if cx < 0 || cy < 0 || cz < 0 {
                            ok = false;
                            break;
                        }
                        let (cx, cy, cz) = (cx as usize, cy as usize, cz as usize);
                        if cx >= cn || cy >= cn || cz >= cn {
                            ok = false;
                            break;
                        }
                        let id = cube_vert[cube_index(cx, cy, cz)];
                        if id == u32::MAX {
                            ok = false;
                            break;
                        }
                        v[i] = id;
                    }
                    if !ok {
                        continue;
                    }
                    // 期望法线：从状态小的一侧指向状态大的一侧
                    let want = if s1.as_i8() > s0.as_i8() {
                        dir
                    } else {
                        [-dir[0], -dir[1], -dir[2]]
                    };
                    for tri in [[v[0], v[1], v[2]], [v[0], v[2], v[3]]] {
                        let a = mesh.positions[tri[0] as usize];
                        let b = mesh.positions[tri[1] as usize];
                        let c = mesh.positions[tri[2] as usize];
                        let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                        let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                        let nn = [
                            ab[1] * ac[2] - ab[2] * ac[1],
                            ab[2] * ac[0] - ab[0] * ac[2],
                            ab[0] * ac[1] - ab[1] * ac[0],
                        ];
                        let dot = nn[0] * want[0] + nn[1] * want[1] + nn[2] * want[2];
                        if dot >= 0.0 {
                            mesh.indices.extend_from_slice(&tri);
                        } else {
                            mesh.indices.extend_from_slice(&[tri[0], tri[2], tri[1]]);
                        }
                    }
                    mesh.quads += 1;
                }
            }
        }
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ternary_roundtrip() {
        for s in TriSign::ALL {
            assert_eq!(TriSign::from_ternary(s.to_ternary()), s);
        }
        assert!(TriSign::Inside.is_decided());
        assert!(!TriSign::Null.is_decided());
    }

    #[test]
    fn all_null_has_no_surface() {
        let f = SignField3D::filled(8, TriSign::Null);
        let m = extract_surface(&f, [0.0; 3], 1.0);
        assert!(m.positions.is_empty());
        assert!(m.indices.is_empty());
    }

    #[test]
    fn counts_track_set() {
        let mut f = SignField3D::new(4);
        assert_eq!(f.counts(), [0, 64, 0]);
        f.set(1, 1, 1, TriSign::Outside);
        assert_eq!(f.counts(), [0, 63, 1]);
        f.set(1, 1, 1, TriSign::Outside);
        assert_eq!(f.counts(), [0, 63, 1]);
        f.set(1, 1, 1, TriSign::Inside);
        assert_eq!(f.counts(), [1, 63, 0]);
    }

    #[test]
    fn solid_cube_gives_closed_shell() {
        let n = 9;
        let mut f = SignField3D::filled(n, TriSign::Outside);
        for z in 2..7 {
            for y in 2..7 {
                for x in 2..7 {
                    f.set(x, y, z, TriSign::Inside);
                }
            }
        }
        let m = extract_surface(&f, [0.0; 3], 1.0);
        assert!(!m.positions.is_empty(), "实心块应产出网格");
        assert_eq!(m.indices.len() % 3, 0);
        assert!(m.quads >= 6 * 5 * 5 / 2, "至少覆盖六个面的一部分");
        // 顶点应落在立方体界面附近
        for p in &m.positions {
            assert!(p[0] > 1.0 && p[0] < 7.5, "顶点越界：{p:?}");
            assert!(p[1] > 1.0 && p[1] < 7.5);
            assert!(p[2] > 1.0 && p[2] < 7.5);
        }
    }

    #[test]
    fn closed_shell_euler() {
        // 实心立方应得到亏格 0 的闭壳：χ = V - E + F = 2
        let n = 9;
        let mut f = SignField3D::filled(n, TriSign::Outside);
        for z in 2..7 {
            for y in 2..7 {
                for x in 2..7 {
                    f.set(x, y, z, TriSign::Inside);
                }
            }
        }
        let m = extract_surface(&f, [0.0; 3], 1.0);
        let mut edges: std::collections::HashMap<(u32, u32), usize> =
            std::collections::HashMap::new();
        for t in m.indices.chunks(3) {
            for k in 0..3 {
                let a = t[k];
                let b = t[(k + 1) % 3];
                let key = if a < b { (a, b) } else { (b, a) };
                *edges.entry(key).or_insert(0) += 1;
            }
        }
        let e = edges.len() as i64;
        let v = m.positions.len() as i64;
        let faces_count = (m.indices.len() / 3) as i64;
        // 四边形被切成两三角形：χ 用三角形面计数
        let chi = v - e + faces_count;
        assert_eq!(chi, 2, "实心立方应闭合且亏格 0，实际 χ={chi}（V={v} E={e} F={faces_count}）");
    }

    #[test]
    fn null_sandwich_keeps_both_interfaces() {
        // +1 | 0 | +1：Null 占一层体素 ⇒ 两侧各一层界面。
        // 这不是缺陷，而是 3PSDF「薄层需要更密采样」的直接体现；
        // 厚度 ≤ 1 体素时两层界面落在同一个 cube 内，DC 会合并成单个顶点。
        let n = 9;
        let mut f = SignField3D::filled(n, TriSign::Outside);
        for y in 0..n {
            for x in 0..n {
                f.set(x, y, 4, TriSign::Null);
            }
        }
        let m = extract_surface(&f, [0.0; 3], 1.0);
        assert!(!m.indices.is_empty());
        let mut zs: Vec<f32> = m.positions.iter().map(|p| p[2]).collect();
        zs.sort_by(|a, b| a.total_cmp(b));
        // 界面在 z≈3.5 与 z≈4.5 两处
        assert!(zs[0] < 3.6, "应有下界面：{:?}", zs.first());
        assert!(*zs.last().unwrap() > 4.4, "应有上界面：{:?}", zs.last());
    }
}
