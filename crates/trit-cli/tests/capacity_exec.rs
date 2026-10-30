//! M5.2 自壳的判决性测试：**REJECT 时子进程必须真的没启动**。
//!
//! 判据不是「打印了拒绝」而是**退出码**：子进程固定以 7 退出，
//! 所以 `Some(7)` = 它跑过，`Some(3)` = 被容量门挡下（3 是 `trit exec` 的拒绝码）。
//! 读数一律用 `TRIT_CAPACITY_FAKE` 测试桩注入，不依赖 CI 机器有没有显卡。

use std::process::{Command, Output};

fn trit_bin() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_trit") {
        return std::path::PathBuf::from(p);
    }
    let mut dir = std::env::current_exe().expect("current exe");
    dir.pop();
    dir.pop();
    dir.join(format!("trit{}", std::env::consts::EXE_SUFFIX))
}

/// 固定以 7 退出的子进程（两平台各一种写法）。
fn child_exit7() -> Vec<String> {
    #[cfg(windows)]
    {
        vec!["cmd".into(), "/C".into(), "exit 7".into()]
    }
    #[cfg(not(windows))]
    {
        vec!["sh".into(), "-c".into(), "exit 7".into()]
    }
}

fn run_exec(extra: &[&str], fake: Option<&str>) -> Output {
    let mut c = Command::new(trit_bin());
    c.arg("exec");
    c.args(extra);
    c.arg("--");
    c.args(child_exit7());
    if let Some(v) = fake {
        c.env("TRIT_CAPACITY_FAKE", v);
    } else {
        c.env_remove("TRIT_CAPACITY_FAKE");
    }
    c.output().expect("spawn trit exec")
}

fn stderr_of(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// 免费 100 MiB，要 1000 MiB × 2 ⇒ 必然 REJECT ⇒ 子进程不得启动。
#[test]
fn reject_must_not_start_child() {
    let out = run_exec(&["--need-mb", "1000"], Some("1000,900,100"));
    assert_eq!(
        out.status.code(),
        Some(3),
        "REJECT 必须用退出码 3（子进程未启动）；若为 7 说明子进程被启动了。stderr={}",
        stderr_of(&out)
    );
    let e = stderr_of(&out);
    assert!(e.contains("REJECT"), "必须打印判决档位：{e}");
    assert!(e.contains("fake"), "测试桩读数必须自标来源：{e}");
}

/// `--force` 越过判决：子进程真的跑（退出码 7）。
#[test]
fn force_overrides_and_runs_child() {
    let out = run_exec(&["--need-mb", "1000", "--force"], Some("1000,900,100"));
    assert_eq!(out.status.code(), Some(7), "stderr={}", stderr_of(&out));
    assert!(stderr_of(&out).contains("--force"), "越过时必须留痕");
}

/// 够用 ⇒ 直接跑。
#[test]
fn ok_runs_child() {
    let out = run_exec(&["--need-mb", "10"], Some("1000,900,100"));
    assert_eq!(out.status.code(), Some(7), "stderr={}", stderr_of(&out));
    assert!(stderr_of(&out).contains("OK"));
}

/// 测不到读数：默认放行（否则非 NVIDIA 机器上工具不可用），`--strict` 下拒跑。
#[test]
fn unknown_is_lenient_by_default_strict_refuses() {
    let out = run_exec(&["--need-mb", "10"], Some("unknown"));
    assert_eq!(
        out.status.code(),
        Some(7),
        "默认应放行：{}",
        stderr_of(&out)
    );

    let out = run_exec(&["--need-mb", "10", "--strict"], Some("unknown"));
    assert_eq!(
        out.status.code(),
        Some(3),
        "--strict 下测不到必须拒跑：{}",
        stderr_of(&out)
    );
}

/// 不给 `--need-mb` = 不做判决，直接执行（且必须明说没做）。
#[test]
fn without_need_runs_and_says_so() {
    let out = run_exec(&[], Some("1000,900,100"));
    assert_eq!(out.status.code(), Some(7), "stderr={}", stderr_of(&out));
    assert!(stderr_of(&out).contains("未做准入判决"));
}
