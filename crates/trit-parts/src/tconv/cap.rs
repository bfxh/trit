//! 卷积栈的通道数上限。
//!
//! `Conv::new(cin, cout)` 内部是三份 `cout·cin·9` 个 f32（`w`/`mw`/`vw`），
//! 外部尺寸（TPC 文件头、`--conv` 参数、模型文本）直接进分配就是 GB 级。
//! 校验必须在分配之前，且先加宽再乘——`usize` 上乘法会绕回一个小数。

/// 每层通道数上限：`2048²×9` 个 f32 = 151 MB/份（`w`/`mw`/`vw` 三份 ≈ 453 MB）。
pub const MAX_CH: usize = 2048;

/// 卷积层尺寸校验，返回权重元素数 `cin·cout·9`。
pub fn check_ch(cin: usize, cout: usize) -> Result<usize, String> {
    if !(1..=MAX_CH).contains(&cin) || !(1..=MAX_CH).contains(&cout) {
        return Err(format!(
            "卷积通道必须在 1..={MAX_CH}（收到 cin={cin} cout={cout}）"
        ));
    }
    cin.checked_mul(cout)
        .and_then(|n| n.checked_mul(9))
        .ok_or_else(|| format!("卷积权重 {cin}×{cout}×9 溢出"))
}
