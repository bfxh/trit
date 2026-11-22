//! 超时终止：杀**进程树**，不只直接子进程。
//!
//! `std::process::Child::kill` 在 Windows 上只结束直接子进程；重写器若是
//! `powershell -File wrapper.ps1` 再拉起子进程，孙进程会继续跑、继续占用管道。
//! 这里在 Unix 上先给子进程开独立进程组（`process_group(0)`）再 `kill(-pgid)`；
//! Windows 上用系统自带 `taskkill /T /F`，失败仍退回直接 kill。

use std::process::{Child, Command};

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

/// 构造外部命令；Unix 上放进独立进程组，便于整组终止。
pub fn command(prog: &str, args: &[String]) -> Command {
    let mut cmd = Command::new(prog);
    cmd.args(args);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd
}

pub fn tree(child: &mut Child) {
    let pid = child.id();
    #[cfg(windows)]
    {
        use std::process::Stdio;
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(unix)]
    {
        // 负 PID = 整个进程组；SIGKILL = 9。即使组已空，后面的 child.kill 兜底。
        // SAFETY: 只传本进程刚 spawn 出的合法 pid 与常量信号；返回值被忽略。
        unsafe {
            kill(-(pid as i32), 9);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}
