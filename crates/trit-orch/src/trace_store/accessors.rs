//! `TraceEventStore` 的只读访问器与水位路径；放在独立文件以守住模块预算。

use std::path::{Path, PathBuf};

use trit_ops::FrameCode;

use super::{TraceAnchor, TraceEventStore};

impl TraceEventStore {
    pub fn strict(&self) -> bool {
        self.strict
    }

    pub fn anchor(&self) -> Option<&TraceAnchor> {
        self.anchor.as_ref()
    }

    /// 当前 ECC 设置（`None` = 关闭）。
    pub fn ecc(&self) -> Option<FrameCode> {
        self.ecc
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn watermark_path(&self) -> PathBuf {
        let mut name = self.path.as_os_str().to_os_string();
        name.push(".watermark");
        PathBuf::from(name)
    }
}
