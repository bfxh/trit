//! CLI 的分发跑在一条**给定栈大小**的线程上，而不是压在进程主线程的默认栈上。
//!
//! 为什么必须这样（2026-11-06 实测）：`main` 里那张 `match cli.command` 有 20 多个分支，
//! clap 的派生在 debug 构建下会把每个分支的临时量都摊进同一个栈帧。同一条命令
//! `trit capacity exec --need-mb 1000`（`tests/capacity_exec.rs` 那 5 条）在
//! 主线程栈 512 KB 下**全部 `has overflowed its stack`**；1 MB（Windows 的默认值）只是刚好躲过，
//! 于是"多加一个子命令"就把三平台的 CI 顶红。栈大小写在这里，就不再取决于平台默认值。

use anyhow::Result;

/// 工作线程的栈：64 MiB。够到不再需要讨论平台默认值，也远小于任何真实内存压力。
pub(crate) const STACK_BYTES: usize = 64 << 20;

/// 在 64 MiB 栈的线程上跑 `f`，并把结果原样带回（panic 按 101 退出，不伪装成成功）。
pub(crate) fn on_big_stack<F>(f: F) -> Result<()>
where
    F: FnOnce() -> Result<()> + Send + 'static,
{
    let worker = std::thread::Builder::new()
        .name("trit-cli".into())
        .stack_size(STACK_BYTES)
        .spawn(f)
        .map_err(|e| anyhow::anyhow!("起不了 {} B 栈的工作线程：{e}", STACK_BYTES))?;
    match worker.join() {
        Ok(result) => result,
        Err(_) => {
            eprintln!("FAIL trit-cli 工作线程 panic（栈 {} B）", STACK_BYTES);
            std::process::exit(101);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每层摊 64 KiB 栈数组：递归 40 层就要 >2.5 MB —— 远超 Windows 主线程默认的 1 MB。
    /// 跑得过 ⇒ 工作线程真的拿到了配给它的栈。
    fn burn(depth: usize, sink: &mut u64) -> u64 {
        if depth == 0 {
            return 0;
        }
        let mut frame = [depth as u64; 8192];
        let acc = burn(depth - 1, sink);
        frame[0] = acc;
        std::hint::black_box(&frame);
        *sink += frame.iter().sum::<u64>();
        *sink
    }

    #[test]
    fn worker_thread_actually_gets_the_big_stack() {
        let result = on_big_stack(|| {
            let mut sink = 0u64;
            burn(40, &mut sink);
            Ok(())
        });
        assert!(result.is_ok(), "64 MiB 栈上跑不完 2.5 MB 递归：{result:?}");
    }

    #[test]
    fn result_is_propagated_verbatim() {
        let ok = on_big_stack(|| Ok(())).is_ok();
        assert!(ok, "Ok 必须原样带回");
        let err = on_big_stack(|| anyhow::bail!("内部错误")).unwrap_err();
        assert!(
            err.to_string().contains("内部错误"),
            "错误信息被吞了：{err}"
        );
    }
}
