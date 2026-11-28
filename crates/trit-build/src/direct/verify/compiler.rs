//! 宿主 C 编译器发现：`CC` 优先，否则试 `cc` / `gcc` / `clang`。

use std::process::Command;

use anyhow::{Result, bail};

pub(super) fn find_compiler() -> Result<String> {
    if let Ok(cc) = std::env::var("CC")
        && !cc.trim().is_empty()
    {
        return Ok(cc);
    }
    for name in ["cc", "gcc", "clang"] {
        if Command::new(name)
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
        {
            return Ok(name.to_string());
        }
    }
    bail!("没有可用的 C 编译器（试过 CC / cc / gcc / clang）")
}
