//! 帧选择、输出路径与线程数解析。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// 帧选择：`all`、半开区间 `a..b`、逗号列表；`--frame N` 是单帧别名。
pub(super) fn parse_frames(spec: &str, total: usize) -> Result<Vec<usize>, String> {
    if total == 0 {
        return Err("数据集没有帧".into());
    }
    let mut out = Vec::new();
    for part in spec.split(',') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        if p.eq_ignore_ascii_case("all") {
            out.extend(0..total);
            continue;
        }
        if let Some((a, b)) = p.split_once("..") {
            let start = if a.trim().is_empty() {
                0
            } else {
                index(a.trim())?
            };
            let end = if b.trim().is_empty() {
                total
            } else {
                index(b.trim())?
            };
            if start >= end {
                return Err(format!("空帧区间: {p}"));
            }
            if end > total {
                return Err(format!("帧区间 {p} 超界（共 {total} 帧）"));
            }
            out.extend(start..end);
        } else {
            let i = index(p)?;
            if i >= total {
                return Err(format!("帧 {i} 超界（共 {total} 帧）"));
            }
            out.push(i);
        }
    }
    if out.is_empty() {
        return Err("没有选中任何帧".into());
    }
    let mut seen = HashSet::new();
    for &f in &out {
        if !seen.insert(f) {
            return Err(format!("帧 {f} 重复"));
        }
    }
    Ok(out)
}

fn index(s: &str) -> Result<usize, String> {
    s.parse::<usize>().map_err(|_| format!("非法帧号: {s}"))
}

/// 多帧输出：`{frame}` 模板，或把 `--out` 当目录；单帧保持原路径。
pub(super) fn frame_paths(out: &str, frames: &[usize]) -> Result<Vec<PathBuf>, String> {
    if out.contains("{frame}") {
        return Ok(frames
            .iter()
            .map(|f| PathBuf::from(out.replace("{frame}", &format!("{f:04}"))))
            .collect());
    }
    if frames.len() == 1 {
        return Ok(vec![PathBuf::from(out)]);
    }
    if out.to_ascii_lowercase().ends_with(".png") {
        return Err("多帧输出请用目录，或在 --out 里放 {frame} 模板".into());
    }
    std::fs::create_dir_all(out).map_err(|e| format!("创建输出目录 {out}: {e}"))?;
    Ok(frames
        .iter()
        .map(|f| Path::new(out).join(format!("frame_{f:04}.png")))
        .collect())
}

/// `--threads`：缺省 1；`auto` / `0` 用可用并行度。
pub(super) fn parse_threads(value: Option<&str>) -> Result<usize, String> {
    let Some(s) = value else {
        return Ok(1);
    };
    if s.eq_ignore_ascii_case("auto") || s == "0" {
        return Ok(std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1));
    }
    s.parse::<usize>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| format!("--threads 非法: {s}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threads_default_to_one_and_accept_auto_or_positive() {
        assert_eq!(parse_threads(None).unwrap(), 1);
        assert!(parse_threads(Some("auto")).unwrap() >= 1);
        assert_eq!(parse_threads(Some("3")).unwrap(), 3);
        assert!(parse_threads(Some("0")).unwrap() >= 1);
    }

    #[test]
    fn rejects_invalid_thread_count() {
        assert!(parse_threads(Some("-1")).is_err());
        assert!(parse_threads(Some("many")).is_err());
    }
}
