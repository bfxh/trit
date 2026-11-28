//! 白平衡参数：色温与绿-洋红色调。

/// 白平衡参数；默认中性。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WhiteBalance {
    /// 色温，单位 K；0 表示关闭。
    pub temperature_k: f32,
    /// 绿-洋红色调，[-1, 1]；0 表示中性。
    pub tint: f32,
}

impl Default for WhiteBalance {
    fn default() -> Self {
        Self {
            temperature_k: 0.0,
            tint: 0.0,
        }
    }
}
