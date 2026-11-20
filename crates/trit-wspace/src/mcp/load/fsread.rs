//! MCP 工作区根、路径限界与只读装载（从 load.rs 拆出，控制行数预算）。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// MCP 显式工作区根（`mcp --root <dir>`）；未设置时退回进程 cwd。
static ROOT: OnceLock<PathBuf> = OnceLock::new();

/// 单次工具调用允许读取的最大输入（32 MiB）。
const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;

/// 工作区根 = 显式 `--root`，否则退回进程 cwd（启动时由 [`refuse_wide_default_root`] 校验）。
pub(crate) fn workspace_root() -> Result<PathBuf, String> {
    if let Some(root) = ROOT.get() {
        return Ok(root.clone());
    }
    std::env::current_dir().map_err(|_| "无法确定工作区根".to_string())
}

/// 设置 `mcp --root <dir>` 的工作区根：必须是存在的目录，且不能是文件系统根/用户主目录。
pub(crate) fn set_root(root: &str) -> Result<(), String> {
    let root = Path::new(root)
        .canonicalize()
        .map_err(|_| format!("工作区根不可用（不存在或非法）：{root}"))?;
    if !root.is_dir() {
        return Err(format!("工作区根不是目录：{}", root.display()));
    }
    if is_wide_root(&root) {
        return Err(format!(
            "拒绝把 `{}` 当作工作区根：请用 `mcp --root <项目目录>` 指定一个子目录",
            root.display()
        ));
    }
    ROOT.set(root)
        .map_err(|_| "工作区根已经设置，不能重复指定".to_string())
}

/// 未显式给 `--root` 时，拒绝以 `/`、盘符根或用户主目录作为默认工作区。
pub(crate) fn refuse_wide_default_root() -> Result<(), String> {
    let cwd = std::env::current_dir()
        .map_err(|_| "无法确定工作区根".to_string())?
        .canonicalize()
        .map_err(|_| "工作区根不可用".to_string())?;
    if is_wide_root(&cwd) {
        return Err(format!(
            "MCP 拒绝以 `{}` 作为默认工作区根：请在项目目录启动，或显式传 `mcp --root <dir>`",
            cwd.display()
        ));
    }
    Ok(())
}

/// `/`、`C:\\`、`$HOME`（或 `%USERPROFILE%`）都不是可接受的工作区根。
fn is_wide_root(path: &Path) -> bool {
    if path.parent().is_none() {
        return true;
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    match home {
        Some(home) => Path::new(&home)
            .canonicalize()
            .map(|home| home == path)
            .unwrap_or(false),
        None => false,
    }
}

/// 只读常规文件：拒绝 FIFO/设备/目录，并给读取量设硬上限（防止阻塞与内存放大）。
pub(crate) fn read_text(path: &Path) -> Result<String, String> {
    let metadata = std::fs::metadata(path).map_err(|_| "读取失败".to_string())?;
    if !metadata.is_file() {
        return Err("只允许读取常规文件（拒绝 FIFO/设备/目录）".to_string());
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err(format!(
            "文件过大（{} 字节，上限 {} 字节）",
            metadata.len(),
            MAX_FILE_BYTES
        ));
    }
    let file = std::fs::File::open(path).map_err(|_| "读取失败".to_string())?;
    let mut text = String::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|_| "读取失败".to_string())?;
    if text.len() as u64 > MAX_FILE_BYTES {
        return Err(format!("文件过大（上限 {MAX_FILE_BYTES} 字节）"));
    }
    Ok(text)
}

/// 把输入路径限制在工作区根内（canonicalize 后比对前缀）；越界 / 不可解析一律拒绝（M19）。
/// 用 `root` 参数便于单测（不依赖进程 cwd）。
pub(crate) fn confined_in(root: &Path, path: &str) -> Result<PathBuf, String> {
    let root = root
        .canonicalize()
        .map_err(|_| "工作区根不可用".to_string())?;
    let candidate = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        root.join(path)
    };
    let real = candidate
        .canonicalize()
        .map_err(|_| "路径不可用（不存在或非法）".to_string())?;
    if real.starts_with(&root) {
        Ok(real)
    } else {
        Err("路径越出工作区范围".into())
    }
}

/// 只允许读工作区根内的文件；错误不回显路径/内容（M19）。
pub(crate) fn confined(path: &str) -> Result<PathBuf, String> {
    confined_in(&workspace_root()?, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confined_rejects_escapes_and_accepts_inside() {
        // M19：工作区根内的相对路径放行；绝对外部路径、`..` 越界一律拒绝。
        let base = std::env::temp_dir().join(format!("trit-mcp-conf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("ws");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/ok.wsp"), "wsp1").unwrap();
        let outside = base.join("outside.txt");
        std::fs::write(&outside, "secret").unwrap();

        assert!(confined_in(&root, "sub/ok.wsp").is_ok(), "根内相对路径放行");
        assert!(
            confined_in(&root, outside.to_str().unwrap()).is_err(),
            "绝对外部路径拒绝"
        );
        assert!(
            confined_in(&root, "../outside.txt").is_err(),
            "`..` 越界拒绝"
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
