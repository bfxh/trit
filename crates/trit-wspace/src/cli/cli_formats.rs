//! CLI `formats`：输出格式注册清单（JSON）。

use super::emit;

pub(super) fn formats(_args: &[String]) -> Result<(), String> {
    emit(&crate::formats::registry_json())
}
