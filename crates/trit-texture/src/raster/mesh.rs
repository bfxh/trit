//! 顶点与网格：位置直接给**裁剪空间**，UV 给第 0 级纹理坐标。

/// 顶点：裁剪空间位置 + UV。
///
/// `clip` 是齐次坐标，光栅化器自己做透视除法；`uv` 是纹理第 0 级的坐标
/// （配 `Wrap::Repeat` 可超出 `[0, 1]`，配图集矩形时 `[0, 1]` 映射到矩形内）。
#[derive(Clone, Copy, Debug)]
pub struct Vertex {
    /// 裁剪空间 `(x, y, z, w)`。
    pub clip: [f32; 4],
    /// 纹理坐标。
    pub uv: [f32; 2],
}

/// 三角网格：顶点表 + 索引表。两条三角形绕向都接受（见 [`crate::rasterize`]）。
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    /// 顶点。
    pub vertices: Vec<Vertex>,
    /// 三角形（顶点下标，每项 3 个）。
    pub indices: Vec<[u32; 3]>,
}

impl Mesh {
    /// 空网格。
    pub fn new() -> Self {
        Self::default()
    }

    /// 三角形个数。
    pub fn triangle_count(&self) -> usize {
        self.indices.len()
    }
}
