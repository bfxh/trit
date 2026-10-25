//! 软件预取提示。
//!
//! 顺序访问本来就有硬件预取器覆盖；这里只提供显式提示，用来做「预取有没有用」
//! 的 A/B。x86_64 走 `_mm_prefetch`，其他架构是空操作。

/// 把 `p` 指向的缓存行预取到 L1。
#[inline(always)]
pub fn prefetch<T>(p: *const T) {
    #[cfg(target_arch = "x86_64")]
    {
        // SAFETY: prefetch 是提示指令，地址非法也不会 fault；x86_64 基线含 SSE。
        unsafe {
            core::arch::x86_64::_mm_prefetch(p as *const i8, core::arch::x86_64::_MM_HINT_T0);
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = p;
    }
}
