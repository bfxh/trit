//! `--verify-c`：用宿主 C 编译器编译 `direct.c`，逐行读 `truth.csv` 对拍。
//!
//! 与 `direct.rs` 的代码生成分开，避免把「产物生成」和「宿主工具链调用」
//! 混在同一个模块里；两者共用 `trit_direct` 的签名契约。

use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
/// 生成 C 驱动器，用 `cc` 编译 `direct.c` + 驱动器，逐行读 `truth.csv` 对拍。
/// 需要 C 编译器（`CC` / `cc` / `gcc` / `clang` 任一）；没有就明确报错，不静默跳过。
pub(crate) fn verify(out_dir: &Path, vars: &[String], expected_rows: usize) -> Result<String> {
    // 相对 `--out` 传给 `current_dir(out_dir)` 后再拼可执行路径会重复目录，先绝对化。
    let out_dir = out_dir
        .canonicalize()
        .with_context(|| format!("解析输出目录 {} 失败", out_dir.display()))?;
    let driver_path = out_dir.join("direct_check.c");
    fs::write(&driver_path, driver_source(vars.len()))
        .with_context(|| format!("写 {} 失败", driver_path.display()))?;
    let exe_name = if cfg!(windows) {
        "direct_check.exe"
    } else {
        "direct_check"
    };
    let exe = out_dir.join(exe_name);
    let compiler = find_compiler()?;
    let output = Command::new(&compiler)
        .current_dir(&out_dir)
        .args([
            "-O2",
            "-std=c99",
            "-o",
            exe_name,
            "direct.c",
            "direct_check.c",
        ])
        .output()
        .with_context(|| format!("调用 C 编译器 {compiler} 失败"))?;
    if !output.status.success() {
        bail!(
            "C 编译失败（{compiler}）：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let run = Command::new(&exe)
        .current_dir(&out_dir)
        .arg("truth.csv")
        .output()
        .with_context(|| format!("运行 {} 失败", exe.display()))?;
    if !run.status.success() {
        bail!(
            "直算对拍失败：{}",
            String::from_utf8_lossy(&run.stderr).trim()
        );
    }
    let stdout = String::from_utf8_lossy(&run.stdout).trim().to_string();
    let rows: usize = stdout
        .strip_prefix("ok rows=")
        .and_then(|value| value.parse().ok())
        .with_context(|| format!("驱动器输出格式不符：{stdout:?}"))?;
    if rows != expected_rows {
        bail!("对拍行数 {rows} 与真值表行数 {expected_rows} 不一致");
    }
    Ok(format!("{stdout}（{compiler}）"))
}

fn find_compiler() -> Result<String> {
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

fn driver_source(nvars: usize) -> String {
    let slots = nvars.max(1);
    format!(
        "#include <stdint.h>\n\
         #include <stdio.h>\n\
         #include <stdlib.h>\n\n\
         int8_t trit_direct(const int8_t *v, unsigned n);\n\n\
         int main(int argc, char **argv) {{\n\
         \x20   if (argc != 2) {{ fprintf(stderr, \"usage: %s truth.csv\\n\", argv[0]); return 2; }}\n\
         \x20   FILE *file = fopen(argv[1], \"r\");\n\
         \x20   if (!file) {{ perror(\"open\"); return 2; }}\n\
         \x20   const unsigned NV = {nvars}u;\n\
         \x20   char line[4096];\n\
         \x20   if (!fgets(line, sizeof line, file)) {{ return 2; }}\n\
         \x20   unsigned long rows = 0, bad = 0;\n\
         \x20   int8_t vars[{slots}];\n\
         \x20   while (fgets(line, sizeof line, file)) {{\n\
         \x20       char *cursor = line;\n\
         \x20       long code = strtol(cursor, &cursor, 10); (void)code;\n\
         \x20       if (*cursor == ',') cursor++;\n\
         \x20       for (unsigned i = 0; i < NV; i++) {{\n\
         \x20           vars[i] = (int8_t)strtol(cursor, &cursor, 10);\n\
         \x20           if (*cursor == ',') cursor++;\n\
         \x20       }}\n\
         \x20       const int want = (int)strtol(cursor, &cursor, 10);\n\
         \x20       const int got = (int)trit_direct(vars, NV);\n\
         \x20       if (got != want) {{\n\
         \x20           if (bad < 5) fprintf(stderr, \"row %lu: got %d want %d\\n\", rows, got, want);\n\
         \x20           bad++;\n\
         \x20       }}\n\
         \x20       rows++;\n\
         \x20   }}\n\
         \x20   fclose(file);\n\
         \x20   if (bad) {{ printf(\"FAIL rows=%lu bad=%lu\\n\", rows, bad); return 1; }}\n\
         \x20   printf(\"ok rows=%lu\\n\", rows);\n\
         \x20   return 0;\n\
         }}\n"
    )
}
