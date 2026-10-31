//! 容错与恢复：误操作可返回（AI 或人类把文件覆盖/替换/删错地方的后悔药）。
//!
//! 三层防线（与 journal 的"追加式不可变"同一哲学，落到文件系统层）：
//!
//! 1. **改动前快照**：[`BackupVault::capture`] 把即将被改的文件镜像进
//!    `<root>/backup/<毫秒时间戳>/` 目录树（相对路径保结构）——先拍照再动刀；
//! 2. **保留期**：[`Retention`] 按份数 + 按天数双上限，[`BackupVault::gc`]
//!    周期清除（可由调用方定时触发）；快照以目录名时间戳为准（纯名字驱动，可测）；
//! 3. **恢复**：[`BackupVault::restore`] 整树拷回，**恢复前自动先 capture 现场**
//!    （恢复本身也可能错，undo 链可以一直续）。
//!
//! 容错纪律：
//! - 根围栏：只收 `base` 内的相对路径，`..` 逃逸与绝对路径拒绝（同 `harvest`）；
//! - 损坏快照**如实上报不静默**（[`Inventory::corrupt`]），gc 可清；
//! - 一切写操作 = 临时文件 + 改名（与 journal/snapshot 同一原子模式）。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 保留策略：双上限，任一触线即回收（gc 时生效）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retention {
    /// 最多保留多少份快照（超出清最老的）。
    pub max_versions: usize,
    /// 快照最长保留时长（秒；0 = 不按时间清）。
    pub max_age_secs: u64,
}

impl Default for Retention {
    fn default() -> Self {
        Self {
            max_versions: 20,
            max_age_secs: 7 * 24 * 3600,
        }
    }
}

/// 一份快照：目录名里的毫秒时间戳 + 收录文件数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotInfo {
    pub stamp_ms: u64,
    pub files: usize,
}

/// 快照清单：合法的 + 损坏的（如实分账，不静默跳过）。
#[derive(Debug, Default)]
pub struct Inventory {
    /// 时间升序。
    pub snapshots: Vec<SnapshotInfo>,
    /// 目录名不是合法时间戳的目录。
    pub corrupt: Vec<String>,
}

/// 备份库：`<root>/backup/<stamp_ms>/<相对路径>` 目录树镜像。
pub struct BackupVault {
    root: PathBuf,
    pub retention: Retention,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl BackupVault {
    /// 打开（或惰性创建）备份库。`root` 通常为 `<项目>/.trit`。
    pub fn open(root: impl AsRef<Path>, retention: Retention) -> Self {
        Self {
            root: root.as_ref().join("backup"),
            retention,
        }
    }

    /// 改动前快照：把 `files`（相对 `base`，或 base 内绝对路径）镜像进新时间戳目录。
    /// 先校验全部路径再动文件系统（fail fast 无残留）；不存在的文件跳过并如实计数。
    pub fn capture(&self, base: &Path, files: &[&Path]) -> io::Result<SnapshotInfo> {
        // 1) 先验：全部路径过围栏（任何非法即整体拒绝，无副作用）
        let mut rels = Vec::with_capacity(files.len());
        for f in files {
            rels.push(self.relativize(base, f)?);
        }
        let stamp = now_ms();
        let dir = self.root.join(stamp.to_string());
        let tmp = self.root.join(format!("{stamp}.tmp"));
        // 2) 后写：原子目录出现（先写 .tmp 再改名）
        fs::create_dir_all(&tmp)?;
        let mut copied = 0usize;
        for rel in &rels {
            let src = base.join(rel);
            if !src.is_file() {
                continue; // 不存在/不是文件：跳过（capture 只管"将要动"的）
            }
            let dst = tmp.join(rel);
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&src, &dst)?;
            copied += 1;
        }
        fs::rename(&tmp, &dir)?;
        Ok(SnapshotInfo {
            stamp_ms: stamp,
            files: copied,
        })
    }

    /// 快照清单（时间升序）+ 损坏目录名。
    pub fn list(&self) -> io::Result<Inventory> {
        let mut inv = Inventory::default();
        if !self.root.is_dir() {
            return Ok(inv);
        }
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            let path = entry.path();
            if !path.is_dir() || name.ends_with(".tmp") {
                continue; // 散文件与操作残留（.tmp）不算快照；残留由 gc 清理
            }
            match name.parse::<u64>() {
                Ok(stamp) => {
                    let files = count_files(&path)?;
                    inv.snapshots.push(SnapshotInfo { stamp_ms: stamp, files });
                }
                Err(_) => inv.corrupt.push(name),
            }
        }
        inv.snapshots.sort_by_key(|s| s.stamp_ms);
        inv.corrupt.sort();
        Ok(inv)
    }

    /// 恢复整份快照到 `base`：先把现场 capture 一份（undo 链可续），再整树拷回。
    /// 返回恢复的文件数。
    pub fn restore(&self, base: &Path, stamp_ms: u64) -> io::Result<usize> {
        let dir = self.root.join(stamp_ms.to_string());
        if !dir.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("快照 {stamp_ms} 不存在"),
            ));
        }
        // 恢复前先拍照：现场里将被覆盖的文件先进备份库
        let victims = collect_rel_paths(&dir)?;
        let victim_paths: Vec<PathBuf> = victims.iter().map(|r| base.join(r)).collect();
        let refs: Vec<&Path> = victim_paths.iter().map(|p| p.as_path()).collect();
        if !refs.is_empty() {
            let _ = self.capture(base, &refs)?;
        }
        let mut n = 0usize;
        for rel in &victims {
            let src = dir.join(rel);
            let dst = base.join(rel);
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&src, &dst)?;
            n += 1;
        }
        Ok(n)
    }

    /// 撤销最近一次"改动"= 恢复最新快照。没有快照返回 Ok(0)。
    pub fn undo_last(&self, base: &Path) -> io::Result<usize> {
        let inv = self.list()?;
        match inv.snapshots.last() {
            Some(s) => self.restore(base, s.stamp_ms),
            None => Ok(0),
        }
    }

    /// 周期清除：超份数清最老、超龄清理；顺带清损坏目录与残留 .tmp。
    /// 返回清除的目录数。
    pub fn gc(&self) -> io::Result<usize> {
        let mut removed = 0usize;
        let inv = self.list()?;
        // 损坏与残留
        for name in &inv.corrupt {
            if fs::remove_dir_all(self.root.join(name)).is_ok() {
                removed += 1;
            }
        }
        // 超龄
        let now = now_ms();
        let mut keep: Vec<u64> = inv
            .snapshots
            .iter()
            .filter(|s| {
                self.retention.max_age_secs == 0
                    || now.saturating_sub(s.stamp_ms) <= self.retention.max_age_secs * 1000
            })
            .map(|s| s.stamp_ms)
            .collect();
        // 超份数：保最新 max_versions 份
        if self.retention.max_versions > 0 && keep.len() > self.retention.max_versions {
            let drop_n = keep.len() - self.retention.max_versions;
            let mut old = keep.clone();
            old.sort_unstable();
            for stamp in old.into_iter().take(drop_n) {
                keep.retain(|&k| k != stamp);
            }
        }
        let kept: std::collections::BTreeSet<u64> = keep.into_iter().collect();
        for s in &inv.snapshots {
            if !kept.contains(&s.stamp_ms)
                && fs::remove_dir_all(self.root.join(s.stamp_ms.to_string())).is_ok() {
                    removed += 1;
                }
        }
        // 残留 .tmp 目录
        if let Ok(entries) = fs::read_dir(&self.root) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if name.ends_with(".tmp") {
                    let _ = fs::remove_dir_all(e.path());
                }
            }
        }
        Ok(removed)
    }

    /// 围栏：收"相对 `base` 的相对路径"或"base 内的绝对路径"；
    /// 拒绝 `..` 逃逸与围栏外的绝对路径。
    fn relativize(&self, base: &Path, f: &Path) -> io::Result<PathBuf> {
        let rel: PathBuf = if f.is_relative() {
            f.to_path_buf()
        } else {
            f.strip_prefix(base)
                .map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("{f:?} 不在 {base:?} 围栏内"),
                    )
                })?
                .to_path_buf()
        };
        for comp in rel.components() {
            match comp {
                std::path::Component::Normal(_) => {}
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("{f:?} 含非法路径分量"),
                    ))
                }
            }
        }
        Ok(rel)
    }
}

/// 递归收集目录下的相对路径（文件）。
fn collect_rel_paths(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    fn walk(root: &Path, cur: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
        for entry in fs::read_dir(cur)? {
            let entry = entry?;
            let p = entry.path();
            if p.is_dir() {
                walk(root, &p, out)?;
            } else {
                out.push(p.strip_prefix(root).unwrap_or(&p).to_path_buf());
            }
        }
        Ok(())
    }
    walk(dir, dir, &mut out)?;
    out.sort();
    Ok(out)
}

fn count_files(dir: &Path) -> io::Result<usize> {
    Ok(collect_rel_paths(dir)?.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "trit-recovery-{tag}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn capture_restore_roundtrip() {
        let base = tmpdir("rt");
        let vault = BackupVault::open(base.join(".trit"), Retention::default());
        fs::write(base.join("a.txt"), "原始A").unwrap();
        fs::create_dir_all(base.join("sub")).unwrap();
        fs::write(base.join("sub/b.txt"), "原始B").unwrap();

        // 改动前拍照
        let info = vault
            .capture(&base, &[Path::new("a.txt"), Path::new("sub/b.txt")])
            .unwrap();
        assert_eq!(info.files, 2);

        // 误操作：覆盖
        fs::write(base.join("a.txt"), "改坏了").unwrap();
        fs::write(base.join("sub/b.txt"), "也坏了").unwrap();

        // 恢复
        let n = vault.restore(&base, info.stamp_ms).unwrap();
        assert_eq!(n, 2);
        assert_eq!(fs::read_to_string(base.join("a.txt")).unwrap(), "原始A");
        assert_eq!(fs::read_to_string(base.join("sub/b.txt")).unwrap(), "原始B");
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn restore_captures_scene_first_undo_chain() {
        let base = tmpdir("undo");
        let vault = BackupVault::open(base.join(".trit"), Retention::default());
        fs::write(base.join("x.txt"), "v1").unwrap();
        let s1 = vault.capture(&base, &[Path::new("x.txt")]).unwrap();
        fs::write(base.join("x.txt"), "v2").unwrap();
        let s2 = vault.capture(&base, &[Path::new("x.txt")]).unwrap();
        fs::write(base.join("x.txt"), "v3（写坏了）").unwrap();

        // 回到 v2
        vault.restore(&base, s2.stamp_ms).unwrap();
        assert_eq!(fs::read_to_string(base.join("x.txt")).unwrap(), "v2");
        // 再回到 v1（undo 链）
        vault.restore(&base, s1.stamp_ms).unwrap();
        assert_eq!(fs::read_to_string(base.join("x.txt")).unwrap(), "v1");

        // 恢复动作自身留下了现场快照（v2 已入档）——回滚链完整
        let inv = vault.list().unwrap();
        assert!(inv.snapshots.len() >= 3, "恢复前的现场应也被 capture");
        let _ = s1;
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn gc_respects_version_and_age_limits() {
        let base = tmpdir("gc");
        let vault = BackupVault::open(
            base.join(".trit"),
            Retention {
                max_versions: 2,
                max_age_secs: 0, // 不按时间清，纯份数
            },
        );
        fs::write(base.join("f.txt"), "x").unwrap();
        for i in 0..4u64 {
            // 手工造带时间戳的快照目录（纯名字驱动，可测）
            let dir = base.join(".trit/backup").join(format!("{}", 1_000 + i));
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("f.txt"), "x").unwrap();
        }
        let removed = vault.gc().unwrap();
        assert_eq!(removed, 2, "4 份保 2 份应清 2 份");
        let inv = vault.list().unwrap();
        assert_eq!(inv.snapshots.len(), 2);
        assert_eq!(inv.snapshots[0].stamp_ms, 1_002, "应保留最新的两份");

        // 超龄清理
        let vault2 = BackupVault::open(
            base.join(".trit2"),
            Retention {
                max_versions: 100,
                max_age_secs: 10,
            },
        );
        let now = now_ms();
        for stamp in [now - 60_000, now - 1_000] {
            let dir = base.join(".trit2/backup").join(stamp.to_string());
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("f.txt"), "x").unwrap();
        }
        let removed2 = vault2.gc().unwrap();
        assert_eq!(removed2, 1, "超龄 1 份应清 1 份");
        let inv2 = vault2.list().unwrap();
        assert_eq!(inv2.snapshots.len(), 1);
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn fence_rejects_escape_and_reports_corrupt() {
        let base = tmpdir("fence");
        let vault = BackupVault::open(base.join(".trit"), Retention::default());
        // 绝对路径在围栏外 → 拒绝
        let outside = if cfg!(windows) { Path::new("C:/Windows") } else { Path::new("/etc") };
        assert!(vault.capture(&base, &[outside]).is_err());

        // 损坏目录如实上报
        fs::create_dir_all(base.join(".trit/backup/not-a-stamp")).unwrap();
        let inv = vault.list().unwrap();
        assert_eq!(inv.corrupt, vec!["not-a-stamp".to_string()]);
        fs::remove_dir_all(&base).unwrap();
    }
}
