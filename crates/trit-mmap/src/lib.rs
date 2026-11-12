//! 只读文件映射（mmap）：Unix 用 `mmap`，Windows 用 `CreateFileMappingW` + `MapViewOfFile`。
//!
//! 用途：大文件（数据集 / 模型 / 指纹表）的一次性只读访问，避免把整文件读进堆。
//!
//! **安全边界**（与所有 mmap 库相同）：映射建立后文件内容被外部进程改写属于未定义行为；
//! 本 crate 只承诺“只读视图”，不阻止别的进程改文件。要改就先复制。
//!
//! 空文件返回空切片（不建立映射；`mmap` 长度为 0 是无效调用）。

#![allow(unsafe_code)]

use std::fs::File;
use std::path::Path;

/// 映射错误。
#[derive(Debug, thiserror::Error)]
pub enum MmapError {
    /// 打开/查询文件失败。
    #[error("IO 错误：{0}")]
    Io(#[from] std::io::Error),
    /// 文件长度超出 `usize`（32 位平台上的大文件）。
    #[error("文件过大（{len} 字节，超出 usize）")]
    TooLarge {
        /// 文件长度。
        len: u64,
    },
    /// 系统映射调用失败。
    #[error("映射失败（系统错误码 {code}）")]
    Map {
        /// 系统错误码。
        code: i32,
    },
}

/// 只读映射文件；`Drop` 时释放映射。
pub struct MappedFile {
    file: File,
    len: usize,
    #[cfg(unix)]
    ptr: *mut libc::c_void,
    #[cfg(windows)]
    view: windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS,
    #[cfg(windows)]
    mapping: windows_sys::Win32::Foundation::HANDLE,
}

// SAFETY: 映射是只读的（PROT_READ / PAGE_READONLY + FILE_MAP_READ），
// 多线程共享同一份只读字节不会产生数据竞争；句柄只在本结构体内部使用并按 Drop 释放。
unsafe impl Send for MappedFile {}
unsafe impl Sync for MappedFile {}

impl MappedFile {
    /// 打开并映射（只读）。
    pub fn open(path: impl AsRef<Path>) -> Result<Self, MmapError> {
        let file = File::open(path.as_ref())?;
        let file_len = file.metadata()?.len();
        let len = usize::try_from(file_len).map_err(|_| MmapError::TooLarge { len: file_len })?;

        #[cfg(unix)]
        {
            if len == 0 {
                return Ok(Self {
                    file,
                    len,
                    ptr: std::ptr::null_mut(),
                });
            }
            use std::os::unix::io::AsRawFd;
            // SAFETY: 长度与文件实际大小一致；私有只读映射；失败返回 MAP_FAILED 已检查。
            let ptr = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    len,
                    libc::PROT_READ,
                    libc::MAP_PRIVATE,
                    file.as_raw_fd(),
                    0,
                )
            };
            if ptr == libc::MAP_FAILED {
                return Err(MmapError::Map {
                    code: std::io::Error::last_os_error().raw_os_error().unwrap_or(-1),
                });
            }
            Ok(Self { file, len, ptr })
        }

        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
            use windows_sys::Win32::System::Memory::{
                CreateFileMappingW, FILE_MAP_READ, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
                PAGE_READONLY,
            };
            if len == 0 {
                return Ok(Self {
                    file,
                    len,
                    view: MEMORY_MAPPED_VIEW_ADDRESS {
                        Value: std::ptr::null_mut(),
                    },
                    mapping: std::ptr::null_mut(),
                });
            }
            let raw = file.as_raw_handle() as HANDLE;
            // SAFETY: 只读映射；文件句柄来自存活的 File。
            let mapping = unsafe {
                CreateFileMappingW(raw, std::ptr::null(), PAGE_READONLY, 0, 0, std::ptr::null())
            };
            if mapping.is_null() {
                return Err(MmapError::Map {
                    code: std::io::Error::last_os_error().raw_os_error().unwrap_or(-1),
                });
            }
            // SAFETY: mapping 刚刚创建且只读；失败时关闭句柄。
            let view = unsafe { MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0) };
            if view.Value.is_null() {
                let code = std::io::Error::last_os_error().raw_os_error().unwrap_or(-1);
                unsafe { CloseHandle(mapping) };
                return Err(MmapError::Map { code });
            }
            Ok(Self {
                file,
                len,
                view,
                mapping,
            })
        }

        #[cfg(not(any(unix, windows)))]
        {
            compile_error!("trit-mmap 只支持 unix / windows");
        }
    }

    /// 视图长度（单独成函数：两个平台路径共用同一处长度来源）。
    fn mapped_len(&self) -> usize {
        self.len
    }

    /// 只读字节视图（生命周期绑定到 `self`）。
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        if self.len == 0 {
            return &[];
        }
        // SAFETY: 映射在结构体存活期间有效；len 与映射长度一致；视图只读。
        #[cfg(unix)]
        unsafe {
            std::slice::from_raw_parts(self.ptr as *const u8, self.mapped_len())
        }
        // SAFETY: 同上（Windows 视图）。
        #[cfg(windows)]
        unsafe {
            std::slice::from_raw_parts(self.view.Value as *const u8, self.mapped_len())
        }
    }

    /// 长度（字节）。
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// 是否为空文件。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 底层文件句柄。
    #[must_use]
    pub fn file(&self) -> &File {
        &self.file
    }
}

impl Drop for MappedFile {
    fn drop(&mut self) {
        if self.len == 0 {
            return;
        }
        #[cfg(unix)]
        {
            if !self.ptr.is_null() {
                // SAFETY: 指针与长度来自本结构体的 mmap 调用，且只在此处释放一次。
                unsafe { libc::munmap(self.ptr, self.len) };
            }
        }
        #[cfg(windows)]
        {
            use windows_sys::Win32::Foundation::CloseHandle;
            use windows_sys::Win32::System::Memory::UnmapViewOfFile;
            if !self.view.Value.is_null() {
                // SAFETY: 视图来自本结构体的 MapViewOfFile，且只在此处解除一次。
                unsafe { UnmapViewOfFile(self.view) };
            }
            if !self.mapping.is_null() {
                // SAFETY: 句柄来自本结构体的 CreateFileMappingW，且只在此处关闭一次。
                unsafe { CloseHandle(self.mapping) };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_path(tag: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("trit-mmap-{}-{tag}-{id}", std::process::id()))
    }

    fn write_temp(tag: &str, bytes: &[u8]) -> PathBuf {
        let path = temp_path(tag);
        let mut file = File::create(&path).unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
        path
    }

    #[test]
    fn maps_known_bytes() {
        let payload: Vec<u8> = (0..=255u8).cycle().take(4096 + 17).collect();
        let path = write_temp("known", &payload);
        {
            let mapped = MappedFile::open(&path).unwrap();
            assert_eq!(mapped.len(), payload.len());
            assert!(!mapped.is_empty());
            assert_eq!(mapped.bytes(), payload.as_slice());
        }
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn empty_file_maps_to_empty_slice() {
        let path = write_temp("empty", &[]);
        {
            let mapped = MappedFile::open(&path).unwrap();
            assert_eq!(mapped.len(), 0);
            assert!(mapped.is_empty());
            assert_eq!(mapped.bytes(), &[] as &[u8]);
        }
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn missing_file_is_structured_io_error() {
        let path = temp_path("missing");
        let err = MappedFile::open(&path).err().expect("必须报错");
        assert!(matches!(err, MmapError::Io(_)), "{err}");
    }

    #[test]
    fn map_and_drop_repeats_cleanly() {
        let payload = b"repeat-me".to_vec();
        let path = write_temp("repeat", &payload);
        for _ in 0..16 {
            let mapped = MappedFile::open(&path).unwrap();
            assert_eq!(mapped.bytes(), payload.as_slice());
        }
        std::fs::remove_file(&path).unwrap();
    }
}
