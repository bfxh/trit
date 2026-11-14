//! WSP2 的 mmap 读取：用 `trit-mmap` 映射文件，按块解码而不把整文件读进内存。
//!
//! 之前 `read_*_io` 走 `seek + read_exact`；这里把同一套块索引算法接到只读映射上：
//! 映射本身不复制数据（按页调入），解码只碰目标行块覆盖的字节。

use std::path::Path;

use trit_mmap::MappedFile;

use crate::error::{Error, Result};
use crate::net::Layer;
use crate::wsp2::BlockIndex;

/// 已映射的 WSP2 工件（只读）。
pub struct MappedArtifact {
    file: MappedFile,
}

impl MappedArtifact {
    /// 映射一个 WSP2 文件。
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let file = MappedFile::open(path).map_err(|error| Error::Wire(format!("mmap: {error}")))?;
        Ok(Self { file })
    }

    /// 映射到的字节切片。
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        self.file.bytes()
    }

    /// 文件长度。
    #[must_use]
    pub fn len(&self) -> usize {
        self.file.len()
    }

    /// 是否空文件。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.file.is_empty()
    }

    /// 按索引读整层。
    pub fn read_layer(&self, index: &BlockIndex, layer: usize) -> Result<Layer> {
        index.read_layer(self.bytes(), layer)
    }

    /// 按索引读行区间（只解码覆盖到的字节）。
    pub fn read_rows(
        &self,
        index: &BlockIndex,
        layer: usize,
        row_start: usize,
        rows: usize,
    ) -> Result<Layer> {
        index.read_rows(self.bytes(), layer, row_start, rows)
    }
}

impl std::fmt::Debug for MappedArtifact {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MappedArtifact")
            .field("len", &self.len())
            .finish()
    }
}
