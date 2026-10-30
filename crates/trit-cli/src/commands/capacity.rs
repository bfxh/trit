//! `capacity` 子命令：系统显存快照 + 三档准入判决（M5.1 首片）。
//!
//! 只做观测与判决，不做拦截——拦截是 M5.2/M5.3，且需用户显式开启（`docs/CAPACITY.md` §10.4/§10.5）。

use anyhow::Result;
use trit_capacity::{Probe, Verdict, judge, judge_unknown, probe_gpu};

/// MiB → 字节。
const MIB: u64 = 1024 * 1024;

pub(crate) fn cmd_capacity(need_mb: Option<u64>, headroom: f32, require: bool) -> Result<()> {
    let probe = probe_gpu();

    match probe {
        Probe::Known(m) => {
            println!(
                "显存来源 {}, 总量 {:.1} MiB, 已用 {:.1} MiB, 可用 {:.1} MiB",
                m.source.as_str(),
                m.total as f64 / MIB as f64,
                m.used as f64 / MIB as f64,
                m.free as f64 / MIB as f64
            );
        }
        Probe::Unknown(why) => {
            println!("显存来源 不可用（{why}）");
        }
    }

    let Some(need_mb) = need_mb else {
        // 只观测：没有负载描述符就不做判决（不猜）。
        println!("未给 --need-mb：只报告快照，不做准入判决");
        return Ok(());
    };

    let need = need_mb.saturating_mul(MIB);
    let j = match probe {
        Probe::Known(m) => judge(need, &m, headroom),
        Probe::Unknown(why) => judge_unknown(need, why, headroom),
    };
    println!(
        "判决 {}（trit {:+#}）, 需求 {:.1} MiB × {:.2} = {:.1} MiB",
        j.verdict.as_str(),
        j.verdict.trit(),
        need as f64 / MIB as f64,
        j.headroom,
        j.required as f64 / MIB as f64
    );
    println!("理由：{}", j.reason);

    if require && j.verdict != Verdict::Ok {
        // fail-closed：--require 下 WARN / REJECT / 未知 一律判不过。
        std::process::exit(1);
    }
    Ok(())
}
