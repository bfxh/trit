//! npz 容器读取：zip 层 + npy 头解析。
//!
//! 旧实现用 `npyz::npz::NpzArchive`（npyz 的 "npz" feature），它把 zip 钉死在
//! ^0.6（Dependabot 两条 zip 告警的源头，npyz 上游无解）。这里改走 npyz 自带的
//! npz-without-feature 模式：本 crate 直连 zip 9，npyz 只当 npy 解析器。

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use npyz::NpyFile;
use zip::ZipArchive;

pub fn open_npz(path: impl AsRef<Path>) -> std::io::Result<ZipArchive<BufReader<File>>> {
    ZipArchive::new(BufReader::new(File::open(path)?)).map_err(std::io::Error::from)
}

/// zip 层的"按名取条目 + npy 头解析"。缺条目映射回旧 `NpzArchive::by_name` 的
/// `None` 语义（"{name} 不存在"），其余错误（含 npy 头解析）走"读 {name} 失败"。
pub fn read_entry<'a, R: std::io::Read + std::io::Seek>(
    npz: &'a mut ZipArchive<R>,
    name: &str,
) -> NpyFile<zip::read::ZipFile<'a, R>> {
    let entry = match npz.by_name(&npyz::npz::file_name_from_array_name(name)) {
        Ok(f) => f,
        Err(zip::result::ZipError::FileNotFound) => panic!("{name} 不存在"),
        Err(e) => panic!("读 {name} 失败：{e}"),
    };
    NpyFile::new(entry).unwrap_or_else(|e| panic!("读 {name} 失败：{e}"))
}
