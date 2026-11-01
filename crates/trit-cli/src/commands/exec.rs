//! `exec` 子命令（M5.2 自壳）：先算账，再决定要不要启动子进程。
//!
//! 退出码约定：
//! - 子进程正常结束 ⇒ **子进程的退出码**（0..=125 原样透传）
//! - **3 = 容量拒绝**（子进程**未启动**）—— 与用法错误（2）和子进程自己的失败区分开
//! - 子进程被信号杀死 / 拿不到退出码 ⇒ 1

use anyhow::Result;
use trit_capacity::{Decision, Probe, decide, judge, judge_unknown, probe_gpu};

const MIB: u64 = 1024 * 1024;
/// 容量拒绝的退出码（子进程未启动）。
pub(crate) const EXIT_REFUSED: i32 = 3;

pub(crate) fn cmd_exec(
    need_mb: Option<u64>,
    headroom: f32,
    force: bool,
    strict: bool,
    argv: &[String],
) -> Result<()> {
    let Some((prog, args)) = argv.split_first() else {
        // clap 已保证至少一个词，这里只是防御。
        anyhow::bail!("exec 需要一个要执行的命令");
    };

    let probe = probe_gpu();
    // 读数一律自带出处（§10.3）；测试桩必须显形，否则假的读数会被当成真机读数。
    match probe {
        Probe::Known(m) => eprintln!(
            "trit exec: 显存来源 {}，可用 {:.0} MiB / 总量 {:.0} MiB，已用 {:.0} MiB",
            m.source.as_str(),
            m.free as f64 / MIB as f64,
            m.total as f64 / MIB as f64,
            m.used as f64 / MIB as f64
        ),
        Probe::Unknown(why) => eprintln!("trit exec: 显存来源 不可用（{why}）"),
    }

    match need_mb {
        None => {
            eprintln!("trit exec: 未给 --need-mb ⇒ 未做准入判决，直接执行");
        }
        Some(mb) => {
            let need = mb.saturating_mul(MIB);
            let j = match probe {
                Probe::Known(m) => judge(need, &m, headroom),
                Probe::Unknown(why) => judge_unknown(need, why, headroom),
            };
            eprintln!(
                "trit exec: 判决 {}（trit {:+}）｜{}",
                j.verdict.as_str(),
                j.verdict.trit(),
                j.reason
            );
            if let Decision::Refuse(j) = decide(j, force, strict) {
                eprintln!(
                    "trit exec: 拒绝启动（退出码 {EXIT_REFUSED}），子进程未运行。\
                     必要时用 --force 越过。理由：{}",
                    j.reason
                );
                std::process::exit(EXIT_REFUSED);
            }
            if force {
                eprintln!("trit exec: --force 已越过判决（读数仍如上，供事后复盘）");
            }
        }
    }

    // stdio 继承：子进程的 stdout/stderr 不被本层包装。
    let status = std::process::Command::new(prog).args(args).status()?;
    std::process::exit(status.code().unwrap_or(1));
}
