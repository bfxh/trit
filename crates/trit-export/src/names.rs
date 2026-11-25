//! 读侧张量重名复核（从 lib.rs 拆出，控制单文件行数）。

use std::collections::BTreeSet;

use crate::Error;

/// 记录已出现的张量名；第二个同名张量即返回 `Error::Duplicate`。
pub(crate) fn check(names: &mut BTreeSet<String>, name: &str) -> Result<(), Error> {
    if names.insert(name.to_owned()) {
        Ok(())
    } else {
        Err(Error::Duplicate(name.to_owned()))
    }
}
