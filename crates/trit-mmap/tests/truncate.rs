//! #589：映射建立后文件被截断，`bytes()` 的越界页会 SIGBUS。
//!
//! 库无法阻止第二个可写句柄截断文件（`MAP_PRIVATE` 不钉住大小），
//! 但至少要让调用方能在读之前复核一次，而不是拿到一个必然崩的视图。

use trit_mmap::MappedFile;
#[cfg(unix)]
use trit_mmap::MmapError;

fn temp_path(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "trit-mmap-{tag}-{}-{:?}.bin",
        std::process::id(),
        std::thread::current().id()
    ))
}

#[test]
fn recheck_len_accepts_intact_file() {
    let path = temp_path("intact");
    std::fs::write(&path, vec![0u8; 4096]).unwrap();
    let mapped = MappedFile::open(&path).unwrap();
    assert_eq!(mapped.len(), 4096);
    assert_eq!(mapped.recheck_len().unwrap(), 4096);
    drop(mapped);
    let _ = std::fs::remove_file(&path);
}

#[cfg(unix)]
#[test]
fn recheck_len_detects_truncation_without_reading_the_view() {
    let path = temp_path("truncated");
    std::fs::write(&path, vec![0u8; 4096]).unwrap();
    let mapped = MappedFile::open(&path).unwrap();

    // 独立可写句柄截断；MappedFile 自己拿到的是 O_RDONLY 句柄，无法 set_len。
    let writer = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    writer.set_len(100).unwrap();
    drop(writer);

    match mapped.recheck_len() {
        Err(MmapError::Truncated {
            mapped: 4096,
            current: 100,
        }) => {}
        other => panic!("应报 Truncated，实际 {other:?}"),
    }
    drop(mapped);
    let _ = std::fs::remove_file(&path);
}

/// Windows 在映射存活期内直接拒绝 `SetEndOfFile`（ERROR_USER_MAPPED_FILE=1224），
/// 所以截断型 SIGBUS 在 Windows 上不可达；这里钉住这条平台事实。
#[cfg(windows)]
#[test]
fn windows_refuses_to_truncate_a_mapped_file() {
    let path = temp_path("windows-mapped");
    std::fs::write(&path, vec![0u8; 4096]).unwrap();
    let mapped = MappedFile::open(&path).unwrap();
    let writer = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    let error = writer.set_len(100).unwrap_err();
    assert_eq!(error.raw_os_error(), Some(1224));
    drop(writer);
    assert_eq!(mapped.recheck_len().unwrap(), 4096);
    drop(mapped);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn empty_file_stays_empty() {
    let path = temp_path("empty");
    std::fs::write(&path, []).unwrap();
    let mapped = MappedFile::open(&path).unwrap();
    assert!(mapped.is_empty());
    assert!(mapped.bytes().is_empty());
    assert_eq!(mapped.recheck_len().unwrap(), 0);
    drop(mapped);
    let _ = std::fs::remove_file(&path);
}
