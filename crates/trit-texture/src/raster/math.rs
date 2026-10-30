//! 最小线性代数：光栅化器只用到 4×4 透视投影与一次矩阵乘向量。
//!
//! 行主序，`mul_vec4` 做 `M · v`；深度按 wgpu/D3D 的 `[0, 1]` 约定。

/// 4×4 矩阵（行主序）。
#[derive(Clone, Copy, Debug)]
pub struct Mat4(pub [[f32; 4]; 4]);

impl Mat4 {
    /// 单位矩阵。
    pub const IDENTITY: Self = Self([
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]);

    /// 右手系透视投影（相机看向 `-z`），深度 `[0, 1]`：近平面 → 0，远平面 → 1。
    ///
    /// `fov_y` 是纵向视场角（弧度），`aspect = 宽/高`。
    pub fn perspective(fov_y: f32, aspect: f32, near: f32, far: f32) -> Self {
        let f = 1.0 / (fov_y * 0.5).tan();
        let d = near - far;
        Self([
            [f / aspect, 0.0, 0.0, 0.0],
            [0.0, f, 0.0, 0.0],
            [0.0, 0.0, far / d, near * far / d],
            [0.0, 0.0, -1.0, 0.0],
        ])
    }

    /// `self · v`。
    pub fn mul_vec4(&self, v: [f32; 4]) -> [f32; 4] {
        let mut out = [0.0f32; 4];
        for (row, slot) in self.0.iter().zip(out.iter_mut()) {
            *slot = row[0] * v[0] + row[1] * v[1] + row[2] * v[2] + row[3] * v[3];
        }
        out
    }
}
