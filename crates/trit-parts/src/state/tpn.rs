//! 断点文件的取数原语：每一次读都先验剩余字节。
//!
//! WHY 先验后分配：`n` / `slots` 来自文件，`vec![0.0f32; n]` 会先按声明分配
//! 再逐项读 —— 一份把 `n` 声明成 u32::MAX 的文件足以让进程 OOM abort。

pub(super) fn need(b: &[u8], at: usize, count: usize) -> Result<(), String> {
    let need = count.checked_mul(4).ok_or("断点长度溢出")?;
    let rest = b.len().checked_sub(at).ok_or("断点文件截断")?;
    if rest < need {
        return Err("断点文件截断".into());
    }
    Ok(())
}

pub(super) fn take_u32(b: &[u8], at: &mut usize) -> Result<u32, String> {
    let s = b.get(*at..*at + 4).ok_or("断点文件截断")?;
    *at += 4;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

pub(super) fn take_u64(b: &[u8], at: &mut usize) -> Result<u64, String> {
    let s = b.get(*at..*at + 8).ok_or("断点文件截断")?;
    *at += 8;
    let mut a = [0u8; 8];
    a.copy_from_slice(s);
    Ok(u64::from_le_bytes(a))
}

/// 读 `count` 个 f32：先验剩余字节再分配。
pub(super) fn take_f(b: &[u8], at: &mut usize, count: usize) -> Result<Vec<f32>, String> {
    need(b, *at, count)?;
    let mut v = vec![0.0f32; count];
    for x in &mut v {
        *x = f32::from_bits(take_u32(b, at)?);
    }
    Ok(v)
}
