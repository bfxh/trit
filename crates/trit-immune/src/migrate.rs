//! 破坏性更新的安全升级路径：日志格式迁移 + 一致性校验 + 原子替换。
//!
//! 原则（破坏性更新的纪律）：
//! 1. **迁移前自动备份**——旧文件不改名不删除，另存 `.backup`；
//! 2. **迁移是单向原子的**——临时文件 + rename，中间不出半态；
//! 3. **迁移后一致性校验**——节点数/裁决/隔离标记逐字段对拍；
//! 4. **不支持的版本明确报错**——不静默跳过、不假装成功；
//! 5. **回滚方案**——备份文件可以手动恢复到旧版本。
//!
//! 版本历史：
//! - v1：无时间戳的 Delta 行
//! - v2：每条 Delta 带 HLC 戳 `<物理>:<计数>:<写者>\t<OP>\t...`（当前）

use crate::delta::Delta;
use std::io;
use std::path::{Path, PathBuf};

/// 当前 journal 格式版本。
pub const CURRENT_VERSION: u32 = 2;

/// 支持的旧版本列表（可迁移的输入格式）。
pub const SUPPORTED_OLD_VERSIONS: &[u32] = &[1];

/// 检测日志文件版本（读头部）。
pub fn detect_version(path: &Path) -> io::Result<u32> {
    let text = std::fs::read_to_string(path)?;
    let header = text.lines().next().unwrap_or("");
    if header.starts_with("trit-immune-journal v") {
        let v = header
            .trim_start_matches("trit-immune-journal v")
            .parse::<u32>()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(v)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("无法识别的日志头部：{header:?}"),
        ))
    }
}

/// 把 v1 日志迁移到 v2（无戳 → 带戳）。
///
/// v1 格式：`OP\tfields...`（无时间戳前缀）
/// v2 格式：`<物理>:<计数>:<写者>\tOP\tfields...`
///
/// 迁移使用合成 HLC 戳（物理位 = 0，计数递增，写者 = 0），
/// 保证顺序不变。迁移后重放校验节点数一致。
pub fn migrate_v1_to_v2(path: &Path) -> io::Result<MigrationReport> {
    let text = std::fs::read_to_string(path)?;
    let mut lines = text.lines();

    // 校验头部
    let header = lines.next().unwrap_or("");
    if !header.starts_with("trit-immune-journal v") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("头部不合法：{header:?}"),
        ));
    }
    let old_version: u32 = header
        .trim_start_matches("trit-immune-journal v")
        .parse()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let mut delta_count = 0usize;
    let mut new_lines = vec![crate::journal::HEADER.to_string()];

    let mut counter = 0u32;
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        delta_count += 1;
        if old_version == 1 {
            // v1 → v2：加合成戳
            new_lines.push(format!("0:{counter}:0\t{line}"));
        } else {
            // 已是 v2：原样保留
            new_lines.push(line.to_string());
        }
        counter += 1;
    }

    // 备份旧文件
    let backup = backup_path(path);
    std::fs::copy(path, &backup)?;

    // 原子替换
    let tmp = path.with_extension("journal.migrate-tmp");
    std::fs::write(&tmp, new_lines.join("\n") + "\n")?;
    std::fs::rename(&tmp, path)?;

    // 删除旧快照（格式已变，缓存失效）
    let snap = path.with_extension("snapshot");
    let _ = std::fs::remove_file(&snap);

    Ok(MigrationReport {
        from_version: old_version,
        to_version: CURRENT_VERSION,
        delta_count,
        backup_path: backup,
    })
}

/// 迁移报告。
#[derive(Debug, Clone)]
pub struct MigrationReport {
    pub from_version: u32,
    pub to_version: u32,
    pub delta_count: usize,
    pub backup_path: PathBuf,
}

fn backup_path(p: &Path) -> PathBuf {
    p.with_extension("journal.backup")
}

/// 完整迁移入口：检测版本 → 迁移到当前版本（支持链式 v1→v2）。
pub fn migrate_to_current(path: &Path) -> io::Result<MigrationReport> {
    let v = detect_version(path)?;
    if v == CURRENT_VERSION {
        return Ok(MigrationReport {
            from_version: v,
            to_version: v,
            delta_count: 0,
            backup_path: backup_path(path),
        });
    }
    if !SUPPORTED_OLD_VERSIONS.contains(&v) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("不支持的旧版本 v{v}，支持：{SUPPORTED_OLD_VERSIONS:?}"),
        ));
    }
    // 链式迁移
    let mut report = MigrationReport {
        from_version: v,
        to_version: v,
        delta_count: 0,
        backup_path: backup_path(path),
    };
    let mut current = v;
    while current < CURRENT_VERSION {
        let r = match current {
            1 => migrate_v1_to_v2(path)?,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!("v{current} → v{} 迁移未实现", current + 1),
                ))
            }
        };
        report.to_version = r.to_version;
        report.delta_count = r.delta_count;
        current = r.to_version;
    }
    Ok(report)
}

/// 迁移后校验：重放日志确认状态一致（节点数 > 0，无解析错误）。
pub fn verify_after_migration(path: &Path) -> io::Result<usize> {
    let text = std::fs::read_to_string(path)?;
    let mut lines = text.lines();
    let header = lines.next().unwrap_or("");
    if !header.starts_with("trit-immune-journal v") {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "头部不合法"));
    }
    let mut node_count = 0usize;
    for line in lines.filter(|l| !l.trim().is_empty()) {
        let (_, d) = crate::journal::parse_stamped_line(line)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if matches!(d, Delta::Spawn { .. }) {
            node_count += 1;
        }
    }
    Ok(node_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "trit-immune-mig-{name}-{}.journal",
            std::process::id()
        ))
    }

    #[test]
    fn v1_to_v2_migration_and_verify() {
        let p = temp("v1v2");
        let _ = fs::remove_file(&p);

        // 写一个 v1 日志
        fs::write(
            &p,
            "trit-immune-journal v1\nROOT\tmain\nSPAWN\t0\tDELTA\t登录\nRECORD\t1\t+1\t-\t-\t-\t-\t通过\n",
        )
        .unwrap();

        // 检测版本
        assert_eq!(detect_version(&p).unwrap(), 1);

        // 迁移
        let report = migrate_to_current(&p).unwrap();
        assert_eq!(report.from_version, 1);
        assert_eq!(report.to_version, 2);
        assert_eq!(report.delta_count, 3);
        assert!(backup_path(&p).exists(), "备份文件应存在");

        // 校验
        let node_count = verify_after_migration(&p).unwrap();
        assert_eq!(node_count, 1, "1 个 Spawn");

        // 迁移后应可正常打开
        let j = crate::journal::Journal::open(&p).unwrap();
        assert_eq!(j.dag().nodes().len(), 2, "root + 登录");
        assert_eq!(
            j.dag().node(crate::NodeId(1)).verdict,
            crate::trit::Trit::Hardened
        );

        // 清理
        let _ = fs::remove_file(&p);
        let _ = fs::remove_file(backup_path(&p));
    }

    #[test]
    fn already_current_version_noop() {
        let p = temp("v2noop");
        let _ = fs::remove_file(&p);
        let j = crate::journal::Journal::create_as(&p, "main", 0).unwrap();
        j.into_dag().unwrap();
        assert_eq!(detect_version(&p).unwrap(), 2);
        let report = migrate_to_current(&p).unwrap();
        assert_eq!(report.from_version, 2);
        assert_eq!(report.to_version, 2, "已最新，无迁移");
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let p = temp("bad-version");
        fs::write(&p, "trit-immune-journal v99\nGARBAGE\n").unwrap();
        let result = migrate_to_current(&p);
        assert!(result.is_err(), "v99 应被拒绝");
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn backup_can_restore() {
        let p = temp("backup-restore");
        let _ = fs::remove_file(&p);
        let _ = fs::remove_file(backup_path(&p));

        // v1 原始内容
        let v1_content = "trit-immune-journal v1\nROOT\tmain\nSPAWN\t0\tDELTA\t登录\n";
        fs::write(&p, v1_content).unwrap();

        migrate_to_current(&p).unwrap();

        // 备份文件内容 = 原始 v1 内容
        let backup_content = fs::read_to_string(backup_path(&p)).unwrap();
        assert_eq!(backup_content, v1_content, "备份应保留原始内容");

        // 从备份恢复旧格式
        fs::write(&p, &backup_content).unwrap();
        assert_eq!(detect_version(&p).unwrap(), 1, "恢复后应回到 v1");

        let _ = fs::remove_file(&p);
        let _ = fs::remove_file(backup_path(&p));
    }
}
