//! 最小线性代数：Vec3 / Mat4（行主序，右乘约定）/ 四元数。
//!
//! 不引外部数学库；口径与 Blender/OpenGL 相机约定对齐（+X 右 +Y 上 −Z 前）。

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub fn subtract(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }

    pub fn dot(self, o: Self) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    pub fn cross(self, o: Self) -> Self {
        Self::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }

    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }

    pub fn normalized(self) -> Self {
        let len = self.length();
        if len <= 1e-12 {
            return Self::ZERO;
        }
        Self::new(self.x / len, self.y / len, self.z / len)
    }
}

/// 4×4，行主序存储（`m[row][col]`），与 transforms.json / cameras.json 的字面排布一致。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat4 {
    pub m: [[f32; 4]; 4],
}

impl Mat4 {
    pub fn identity() -> Self {
        let mut m = [[0.0f32; 4]; 4];
        for (i, row) in m.iter_mut().enumerate() {
            row[i] = 1.0;
        }
        Self { m }
    }

    /// 从 16 个行主序元素构造（cameras.json / transforms.json 的 transform_matrix 排布）。
    pub fn from_rows_flat(rows: [f32; 16]) -> Self {
        let mut m = [[0.0f32; 4]; 4];
        for (i, v) in rows.iter().enumerate() {
            m[i / 4][i % 4] = *v;
        }
        Self { m }
    }

    /// 平移分量（第 4 列）。
    pub fn translation(&self) -> Vec3 {
        Vec3::new(self.m[0][3], self.m[1][3], self.m[2][3])
    }

    /// 3×3 旋转部分。
    pub fn rot3(&self) -> [[f32; 3]; 3] {
        [
            [self.m[0][0], self.m[0][1], self.m[0][2]],
            [self.m[1][0], self.m[1][1], self.m[1][2]],
            [self.m[2][0], self.m[2][1], self.m[2][2]],
        ]
    }

    /// 世界点 → 相机空间：`p_cam = Rᵀ (p_w − t)`。
    pub fn world_to_cam(&self, p: Vec3) -> Vec3 {
        let r = self.rot3();
        let d = p.subtract(self.translation());
        Vec3::new(
            r[0][0] * d.x + r[1][0] * d.y + r[2][0] * d.z,
            r[0][1] * d.x + r[1][1] * d.y + r[2][1] * d.z,
            r[0][2] * d.x + r[1][2] * d.y + r[2][2] * d.z,
        )
    }
}

/// 单位化 xyzw 四元数（3DGS PLY 存储口径，rot 属性无需归一化时按原值使用）。
pub fn quat_to_mat3(q: [f32; 4]) -> [[f32; 3]; 3] {
    let norm = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    let (x, y, z, w) = if norm > 1e-12 {
        (q[0] / norm, q[1] / norm, q[2] / norm, q[3] / norm)
    } else {
        (0.0, 0.0, 0.0, 1.0) // 退化四元数当单位旋转
    };
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - w * z),
            2.0 * (x * z + w * y),
        ],
        [
            2.0 * (x * y + w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - w * x),
        ],
        [
            2.0 * (x * z - w * y),
            2.0 * (y * z + w * x),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ]
}

/// Σ = R S Sᵀ Rᵀ，S = diag(scale)。返回对称 3×3。
pub fn covariance(rot: [f32; 4], scale: [f32; 3]) -> [[f32; 3]; 3] {
    let r = quat_to_mat3(rot);
    let mut m = [[0.0f32; 3]; 3]; // M = R·S
    for i in 0..3 {
        for j in 0..3 {
            m[i][j] = r[i][j] * scale[j];
        }
    }
    let mut sigma = [[0.0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            let mut acc = 0.0;
            for (a, b) in m[i].iter().zip(m[j].iter()) {
                acc += a * b;
            }
            sigma[i][j] = acc;
        }
    }
    sigma
}

#[cfg(test)]
mod tests;
