//! 清单路径围栏（从 `manifest.rs` 拆出，守单文件行数预算）。

use std::path::{Component, Path, PathBuf};

pub(super) fn resolve(base: &Path, p: String) -> Result<PathBuf, String> {
    let rel = PathBuf::from(&p);
    // Windows 上 `/abs/x` 是"root-relative"而不是 absolute（is_absolute 为 false），
    // 但语义上仍然不是从包目录出发的相对路径，所以 has_root 也要算进来。
    // 绝对路径一律拒绝：即使清单作者显式写了，也不能让打包器读包外文件。
    if rel.is_absolute() || rel.has_root() {
        return Err(format!("清单里的路径是绝对路径（拒绝）：{p}"));
    }
    let joined = base.join(&rel);
    // 词法折叠 . 与 ..，再跟"包目录的上一级"比 —— 不读盘，所以对不存在的路径
    // 也成立。真实产物的清单就写在 `../objects.manifest.json`（清单在包目录的上
    // 级），所以 `..` 本身合法，越出包目录才不合法。
    let norm = fold(&joined);
    let fence = fold(base.parent().unwrap_or(base));
    if !norm.starts_with(&fence) {
        return Err(format!("清单里的路径越出包目录：{p}"));
    }
    // 词法围栏不够：包内 symlink 可以指向包外，`std::fs::read` 会跟随它。
    // 目标存在时用 canonicalize 解析所有 symlink，并把结果钉在包根内；返回
    // canonical 路径，后续 read 不再走可被替换的 symlink。
    if norm.symlink_metadata().is_ok() {
        let canonical = norm
            .canonicalize()
            .map_err(|error| format!("清单路径无法解析符号链接（{p}）：{error}"))?;
        let canonical_fence = fence
            .canonicalize()
            .map_err(|error| format!("清单包目录无法解析（{}）：{error}", fence.display()))?;
        if !canonical.starts_with(&canonical_fence) {
            return Err(format!("清单里的路径经符号链接越出包目录：{p}"));
        }
        return Ok(canonical);
    }
    // 不存在：保留词法路径，让后续 read 如实报 I/O 错误。
    Ok(joined)
}

/// 纯词法折叠 `.` 与 `..`，不触碰文件系统。
fn fold(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_paths_cannot_escape_the_package_dir() {
        let base = Path::new("pkg/meshes");
        // 真实产物就是这样引用上级的对象清单，必须放行。
        assert!(resolve(base, "../objects.manifest.json".into()).is_ok());
        assert!(resolve(base, "obj-1.obj".into()).is_ok());
        assert!(resolve(base, "sub/obj-1.obj".into()).is_ok());
        // 越出包目录就不行。
        assert!(resolve(base, "../../etc/passwd".into()).is_err());
        assert!(resolve(base, "a/../../../etc/passwd".into()).is_err());
        // 绝对路径是包外引用，必须拒绝（修复前这里返回 Ok）。
        assert!(resolve(base, "/abs/obj-1.obj".into()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_rejected() {
        let root = std::env::temp_dir().join(format!("trit-glb-manifest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let base = root.join("pkg/meshes");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(root.join("outside.txt"), "secret").unwrap();
        std::fs::write(root.join("pkg/inside.txt"), "ok").unwrap();
        std::os::unix::fs::symlink(root.join("outside.txt"), base.join("escape.txt")).unwrap();
        std::os::unix::fs::symlink(root.join("pkg/inside.txt"), base.join("inside.txt")).unwrap();

        assert!(
            resolve(&base, "escape.txt".into()).is_err(),
            "包内 symlink 指向包外必须拒绝"
        );
        assert!(
            resolve(&base, "inside.txt".into()).is_ok(),
            "包内 symlink 仍可解析"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
