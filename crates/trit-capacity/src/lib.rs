//! 系统容量观测与准入判决（M5.1 首片）。
//!
//! 设计正本：`docs/CAPACITY.md`（§10.2 四层能力、§10.3 系统全局、§10.5 默认开启）。
//! 本 crate 只做 **L1 观测**（系统级 GPU 显存）与 **L3 判决**（三档 + headroom 水位），
//! 不做 L4 执行层（拦截启动）——那是 M5.2/M5.3，且需用户显式开启。
//!
//! 三条纪律：
//! 1. **零依赖**：只用 std，用 `nvidia-smi` / sysfs 这类平台既有事实源，不引第三方 crate。
//! 2. **测不到就报 unknown，绝不猜**（§6/§10.3）：`Probe::Unknown` 是合法结果。
//! 3. **判决是三值的**：`Ok/Warn/Reject` ↔ `+1/0/−1`，多资源时用 meet（取最小）合并——
//!    这正是三进制在「准入控制」上的原生语义：只要有一项不过，整体就不过。

use std::process::Command;

/// 显存数据的来源。读数必须自带出处（§10.3 的可复算要求）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemSource {
    /// NVIDIA 驱动自带的 `nvidia-smi`（Windows / Linux 通用）。
    NvidiaSmi,
    /// Linux DRM sysfs（`/sys/class/drm/card*/device/mem_info_vram_*`）。
    LinuxDrm,
}

impl MemSource {
    pub fn as_str(self) -> &'static str {
        match self {
            MemSource::NvidiaSmi => "nvidia-smi",
            MemSource::LinuxDrm => "linux-drm-sysfs",
        }
    }
}

/// 系统级显存快照（字节）。`free` 是**驱动口径**，WDDM 超订下偏乐观（§10.3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuMemory {
    pub total: u64,
    pub used: u64,
    pub free: u64,
    pub source: MemSource,
}

/// 探针结果：测不到也是一种结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    Known(GpuMemory),
    /// 没有任何可用事实源。`why` 给人看，判定必须据此给 WARN/拒绝而不是当成 0 用。
    Unknown(&'static str),
}

/// 三值判决：`+1 = Ok`、`0 = Warn`、`-1 = Reject`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    Warn,
    Reject,
}

impl Verdict {
    /// 映射到三值（与 trit 的 +1/0/−1 同序）。
    pub fn trit(self) -> i8 {
        match self {
            Verdict::Ok => 1,
            Verdict::Warn => 0,
            Verdict::Reject => -1,
        }
    }

    /// 三值 meet（取最小）：任一资源不过，整体就不过。
    pub fn meet(self, other: Verdict) -> Verdict {
        if self.trit() <= other.trit() {
            self
        } else {
            other
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Ok => "OK",
            Verdict::Warn => "WARN",
            Verdict::Reject => "REJECT",
        }
    }
}

/// 一次准入判决的完整账目（可打印、可复算）。
#[derive(Debug, Clone)]
pub struct Judgment {
    pub verdict: Verdict,
    /// 负载需要的字节数。
    pub need: u64,
    /// 驱动报告的可用字节数；`None` = 测不到。
    pub free: Option<u64>,
    /// headroom 水位倍数（≥1）。
    pub headroom: f32,
    /// `ceil(need × headroom)`：为竞争留出的要求值。
    pub required: u64,
    pub reason: String,
}

/// 把 headroom 规范到 ≥1.0（小于 1 的要求等于「允许刚好占满」，那是事故配方）。
pub fn normalize_headroom(headroom: f32) -> f32 {
    if headroom.is_finite() && headroom >= 1.0 {
        headroom
    } else {
        1.0
    }
}

/// `ceil(need × headroom)`，用 f64 中转避免 u64 溢出前的乘法截断。
fn required_with(need: u64, headroom: f32) -> u64 {
    let h = normalize_headroom(headroom) as f64;
    let want = need as f64 * h;
    if want >= u64::MAX as f64 {
        u64::MAX
    } else {
        want.ceil() as u64
    }
}

/// 单资源判决。
///
/// - `free ≥ required` ⇒ OK（含竞争余量）
/// - `free ≥ need` 但 `< required` ⇒ WARN（能跑，余量不足）
/// - `free < need` ⇒ REJECT
pub fn judge(need: u64, gpu: &GpuMemory, headroom: f32) -> Judgment {
    let headroom = normalize_headroom(headroom);
    let required = required_with(need, headroom);
    let free = gpu.free;
    let (verdict, reason) = if free >= required {
        (
            Verdict::Ok,
            format!("可用 {free} B ≥ 需求 {required} B（{need} B × {headroom:.2}）"),
        )
    } else if free >= need {
        (
            Verdict::Warn,
            format!(
                "能跑但余量不足：可用 {free} B < 需求 {required} B（{need} B × {headroom:.2}），\
                 只够 {:.2}× 峰值",
                free as f64 / need.max(1) as f64
            ),
        )
    } else {
        (
            Verdict::Reject,
            format!("可用 {free} B < 峰值 {need} B，必然爆"),
        )
    };
    Judgment {
        verdict,
        need,
        free: Some(free),
        headroom,
        required,
        reason,
    }
}

/// 测不到时的判决：**WARN**（不猜、也不当绿灯）。
///
/// `--require` 模式下调用方应按「WARN 即不过」处理（fail-closed）。
pub fn judge_unknown(need: u64, why: &str, headroom: f32) -> Judgment {
    let headroom = normalize_headroom(headroom);
    Judgment {
        verdict: Verdict::Warn,
        need,
        free: None,
        headroom,
        required: required_with(need, headroom),
        reason: format!("显存读数不可用（{why}）⇒ 不做放行判决；--require 下按不过处理"),
    }
}

/// 探测系统级显存。顺序：`nvidia-smi` → Linux DRM sysfs → Unknown。
pub fn probe_gpu() -> Probe {
    if let Some(m) = probe_nvidia_smi() {
        return Probe::Known(m);
    }
    #[cfg(target_os = "linux")]
    if let Some(m) = probe_linux_drm() {
        return Probe::Known(m);
    }
    Probe::Unknown("没有可用的显存事实源（nvidia-smi 缺失且非 Linux sysfs 环境）")
}

const MIB: u64 = 1024 * 1024;

/// `nvidia-smi --query-gpu=memory.total,memory.used,memory.free --format=csv,noheader,nounits`
/// 输出形如 `8188, 1234, 6954`（MiB）。多卡时取第一张（M5.1 只做单卡口径）。
fn probe_nvidia_smi() -> Option<GpuMemory> {
    let out = Command::new("nvidia-smi")
        .args([
            "--query-gpu=memory.total,memory.used,memory.free",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().next()?;
    let mut it = line.split(',').map(|s| s.trim().parse::<u64>().ok());
    let total = it.next()??;
    let used = it.next()??;
    let free = it.next()??;
    Some(GpuMemory {
        total: total * MIB,
        used: used * MIB,
        free: free * MIB,
        source: MemSource::NvidiaSmi,
    })
}

#[cfg(target_os = "linux")]
fn probe_linux_drm() -> Option<GpuMemory> {
    use std::fs;
    let dir = fs::read_dir("/sys/class/drm").ok()?;
    for entry in dir.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let base = entry.path().join("device");
        let read =
            |f: &str| -> Option<u64> { fs::read_to_string(base.join(f)).ok()?.trim().parse().ok() };
        if let (Some(total), Some(used)) = (read("mem_info_vram_total"), read("mem_info_vram_used"))
        {
            return Some(GpuMemory {
                total,
                used,
                free: total.saturating_sub(used),
                source: MemSource::LinuxDrm,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gpu(total: u64, used: u64) -> GpuMemory {
        GpuMemory {
            total,
            used,
            free: total - used,
            source: MemSource::NvidiaSmi,
        }
    }

    #[test]
    fn ok_when_free_covers_headroom() {
        let g = gpu(1000, 400); // free = 600
        let j = judge(300, &g, 2.0); // required = 600
        assert_eq!(j.verdict, Verdict::Ok, "{}", j.reason);
        assert_eq!(j.required, 600);
    }

    #[test]
    fn warn_when_only_bare_peak_fits() {
        let g = gpu(1000, 400); // free = 600
        let j = judge(400, &g, 2.0); // required = 800 > 600，但 need(400) ≤ free
        assert_eq!(j.verdict, Verdict::Warn, "{}", j.reason);
        assert!(j.reason.contains("余量不足"));
    }

    #[test]
    fn reject_when_peak_exceeds_free() {
        let g = gpu(1000, 400); // free = 600
        let j = judge(601, &g, 1.0);
        assert_eq!(j.verdict, Verdict::Reject, "{}", j.reason);
    }

    #[test]
    fn boundary_free_equals_required_is_ok() {
        let g = gpu(1000, 400);
        let j = judge(600, &g, 1.0);
        assert_eq!(j.verdict, Verdict::Ok, "恰等于可用量应放行（不保守到失效）");
    }

    #[test]
    fn headroom_below_one_is_clamped() {
        let g = gpu(1000, 400);
        let j = judge(600, &g, 0.5);
        assert_eq!(j.headroom, 1.0, "headroom<1 等于允许占满，必须夹到 1.0");
        assert_eq!(j.verdict, Verdict::Ok);
    }

    #[test]
    fn nan_headroom_is_clamped() {
        let g = gpu(1000, 400);
        let j = judge(100, &g, f32::NAN);
        assert_eq!(j.headroom, 1.0);
    }

    #[test]
    fn unknown_is_warn_not_ok() {
        let j = judge_unknown(123, "no source", 2.0);
        assert_eq!(j.verdict, Verdict::Warn, "测不到绝不能当绿灯");
        assert_eq!(j.free, None);
    }

    /// 金丝雀：判决必须能判红（否则这道门是装饰）。
    #[test]
    fn canary_absurd_request_is_rejected() {
        let g = gpu(8 * 1024 * MIB, 1024 * MIB); // 8 GB 卡，用掉 1 GB
        let j = judge(64 * 1024 * MIB, &g, 1.0); // 要 64 GB
        assert_eq!(
            j.verdict,
            Verdict::Reject,
            "100× 越界样本未被判红 ⇒ 判决失效"
        );
        assert_eq!(j.verdict.trit(), -1);
    }

    #[test]
    fn meet_is_min_over_trits() {
        assert_eq!(Verdict::Ok.meet(Verdict::Warn), Verdict::Warn);
        assert_eq!(Verdict::Warn.meet(Verdict::Reject), Verdict::Reject);
        assert_eq!(Verdict::Ok.meet(Verdict::Ok), Verdict::Ok);
        // 交换律
        assert_eq!(
            Verdict::Reject.meet(Verdict::Ok),
            Verdict::Ok.meet(Verdict::Reject)
        );
    }

    #[test]
    fn required_saturates_instead_of_overflowing() {
        let g = gpu(u64::MAX, 0);
        let j = judge(u64::MAX, &g, 2.0);
        assert_eq!(j.required, u64::MAX, "溢出必须饱和，不能回绕成小值");
    }

    /// 真机探针：只断言「不 panic、结果自洽」，不对具体读数下结论。
    #[test]
    fn probe_is_self_consistent() {
        match probe_gpu() {
            Probe::Known(m) => {
                assert!(m.total >= m.used, "total 必须 ≥ used");
                assert!(m.free <= m.total, "free 不能超过总量");
                // 注意：**不能**断言 free == total - used。本机实测 nvidia-smi 的 free 与
                // (total - used) 差 239 MiB（驱动保留 / 共享内存口径），这正是 §10.3
                // 「可用显存是驱动口径、偏乐观」的实证。判决只用 free，不自造减法。
            }
            Probe::Unknown(why) => assert!(!why.is_empty(), "unknown 必须给出原因"),
        }
    }
}
