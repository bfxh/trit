//! 高斯资产加载：`.spz` 直读（需 spz feature），否则按 PLY 解析。

use std::path::Path;

use trit_splat::ply::{RawGaussian, parse_ply};

pub(super) fn load_gaussians(path: &str) -> Result<Vec<RawGaussian>, String> {
    if path.to_ascii_lowercase().ends_with(".spz") {
        #[cfg(feature = "spz")]
        {
            return trit_splat::spz::load_spz(Path::new(path))
                .map_err(|e| format!("SPZ 解析: {e}"));
        }
        #[cfg(not(feature = "spz"))]
        {
            return Err("读 .spz 需要构建时启用 spz feature".into());
        }
    }
    let bytes = std::fs::read(path).map_err(|e| format!("读 ply 失败 {path}: {e}"))?;
    parse_ply(&bytes).map_err(|e| format!("PLY 解析: {e}"))
}
