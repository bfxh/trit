//! 超时终止：杀**进程树**，不只直接子进程。
//!
//! `std::process::Child::kill` 在 Windows 上只结束直接子进程；重写器若是
//! `powershell -File wrapper.ps1` 再拉起子进程，孙进程会继续跑、继续占用管道。
//! 这里在 Unix 上先给子进程开独立进程组（`process_group(0)`）再 `kill(-pgid)`；
//! Windows 上用系统自带 `taskkill /T /F`，失败仍退回直接 kill。

use std::process::{Child, Command};

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
        // 负 PID = 整个进程组；走 shell 内建 kill（POSIX 语义），避免 unsafe。
        use std::process::Stdio;
        let signal = format!("kill -9 -{pid}");
        let _ = Command::new("sh")
            .args(["-c", &signal])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}
