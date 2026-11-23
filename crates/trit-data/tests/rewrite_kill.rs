//! Windows 超时回归：只杀直接子进程会留下孙进程，这里锁住“整棵树都停”。
//!
//! 夹具：`powershell` 包装器先拉起一个 `Start-Sleep 20` 的孙进程，把 PID 写文件，
//! 自己再睡 10s；`--timeout 2` 触发后，孙进程必须在数秒内消失。
//! 非 Windows 由 Unix 进程组路径覆盖，本文件跳过。

#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("时钟")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("trit-kill-{tag}-{nanos}"));
        fs::create_dir_all(&path).expect("建临时目录");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn fixture_line() -> String {
    let final_text = "重复句子。".repeat(80);
    format!(
        "{{\"schema\":1,\"task_id\":\"kill-fixture\",\"model\":\"m\",\
         \"steps\":[{{\"role\":\"assistant\",\"i\":0,\"action\":{{\"tool\":\"bash\",\"args\":{{}}}}}}],\
         \"outcome\":{{\"verifier\":\"manual\",\"passed\":true,\"evidence\":[\"sha256:{}\"],\
         \"final\":\"{final_text}\"}},\"cost\":{{\"tokens_in\":1,\"tokens_out\":1,\
         \"tool_calls\":0,\"ms\":0}}}}",
        "a".repeat(64)
    )
}

fn wrapper(dir: &Path) -> PathBuf {
    let script = dir.join("wrapper.ps1");
    let pid_file = dir.join("grandchild.pid");
    let text = format!(
        "$null = [Console]::In.ReadToEnd()\n\
         $p = Start-Process -FilePath powershell -ArgumentList \"-NoProfile\",\"-Command\",\"Start-Sleep -Seconds 20\" -PassThru\n\
         Set-Content -Path \"{}\" -Value $p.Id\n\
         Start-Sleep -Seconds 10\n",
        pid_file.display()
    );
    fs::write(&script, text).expect("写包装器");
    script
}

fn alive(pid: u32) -> bool {
    let filter = format!("PID eq {pid}");
    let output = Command::new("tasklist")
        .args(["/FI", &filter, "/NH"])
        .output()
        .expect("tasklist");
    String::from_utf8_lossy(&output.stdout).contains(&pid.to_string())
}

#[test]
fn timeout_kills_the_whole_process_tree() {
    let dir = TempDir::new("tree");
    let input = dir.path().join("in.jsonl");
    fs::write(&input, fixture_line()).expect("写输入");
    let script = wrapper(dir.path());
    let out = dir.path().join("out");
    let status = Command::new(env!("CARGO_BIN_EXE_trit-data"))
        .args([
            "rewrite",
            "--jsonl",
            &input.display().to_string(),
            "--cmd",
            &format!("powershell -NoProfile -File \"{}\"", script.display()),
            "--timeout",
            "2",
            "--out",
            &out.display().to_string(),
        ])
        .output()
        .expect("启动 trit-data rewrite");
    assert!(
        status.status.success(),
        "rewrite 退出失败：{}",
        String::from_utf8_lossy(&status.stderr)
    );

    let pid_file = dir.path().join("grandchild.pid");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !pid_file.exists() && Instant::now() < deadline {
        sleep(Duration::from_millis(50));
    }
    let pid: u32 = fs::read_to_string(&pid_file)
        .expect("孙进程 PID 未落盘")
        .trim()
        .parse()
        .expect("PID 不是数字");

    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(pid) && Instant::now() < deadline {
        sleep(Duration::from_millis(100));
    }
    assert!(!alive(pid), "孙进程 {pid} 在超时后仍存活");
}
