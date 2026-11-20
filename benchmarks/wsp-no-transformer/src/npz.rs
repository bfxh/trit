//! NPZ 读取：npz 就是一个装了 `.npy` 的 zip。
//!
//! 为什么不用 `npyz` 的 `npz` feature：那把 zip 钉在 0.6.x，命中
//! GHSA-m665-v7pm-7grj（critical，修复 9.0.5）与 GHSA-8hxf-qr8p-62x4（medium，修复 9.0.0）。
//! 这里自己用 zip 9 读容器，`.npy` 的解析仍交给 `npyz::NpyFile`，对外语义不变。

use std::fs::File;
use std::io::{self, BufReader};
use std::path::Path;

use npyz::NpyFile;
use zip::read::ZipFile;
use zip::result::ZipError;
use zip::ZipArchive;

pub struct Npz {
    archive: ZipArchive<BufReader<File>>,
}

impl Npz {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = File::open(path)?;
        Ok(Self {
            archive: ZipArchive::new(BufReader::new(file))?,
        })
    }

    /// 与 `npyz::npz::NpzArchive::by_name` 同语义：传数组名（不带 `.npy`），大小写敏感。
    #[allow(clippy::type_complexity)]
    pub fn by_name(
        &mut self,
        name: &str,
    ) -> io::Result<Option<NpyFile<ZipFile<'_, BufReader<File>>>>> {
        let file_name = npyz::npz::file_name_from_array_name(name);
        match self.archive.by_name(&file_name) {
            Ok(file) => Ok(Some(NpyFile::new(file)?)),
            Err(ZipError::FileNotFound) => Ok(None),
            Err(ZipError::Io(error)) => Err(error),
            Err(error) => Err(io::Error::new(io::ErrorKind::InvalidData, error.to_string())),
        }
    }
}
