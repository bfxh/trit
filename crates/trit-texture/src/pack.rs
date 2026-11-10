//! 纹理族（A1/A2）的共用件：语料枚举、内容级指纹、逐图指标与采样一致性。
//!
//! 这里只做**纯计算**；读写 PNG/TTX 与清单在 `bin/ttx-pack.rs`、`bin/ttx-verify.rs`。
//! 「采样一致性」= 压缩域采样器在纹素中心必须与整图解码逐字节一致，且 repeat 寻址
//! 跨边界回到同一纹素；它验证的是**两条解码路径说的是同一件事**，不是画质指标。

use std::path::{Path, PathBuf};

use trit_core::fingerprint::{file_sha256, sha256_hex};

use crate::sample::{Sampler, Wrap};
use crate::ttx2::from_ttx2;

/// 一份语料的内容级指纹（逐文件 SHA-256 + 规范序列总哈希）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fp {
    pub files: usize,
    pub bytes: u64,
    pub hash: String,
}

/// 目录下排序后的 `*.png`（A1 与 A2 必须看到同一批文件）。
pub fn list_pngs(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("读不了 {}: {e}", dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "png"))
        .collect();
    v.sort();
    Ok(v)
}

/// 语料指纹：`文件名 文件哈希` 的规范序列再做一次总哈希。
pub fn fingerprint_pngs(files: &[PathBuf]) -> Result<Fp, String> {
    let mut lines = Vec::with_capacity(files.len());
    let mut bytes = 0u64;
    for p in files {
        let name = p
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| format!("文件名非法: {}", p.display()))?;
        let h = file_sha256(p)?;
        bytes += std::fs::metadata(p)
            .map_err(|e| format!("{}: {e}", p.display()))?
            .len();
        lines.push(format!("{name} {h}"));
    }
    Ok(Fp {
        files: files.len(),
        bytes,
        hash: sha256_hex(lines.join("\n").as_bytes()),
    })
}

/// 一行的逐图记录（`pack.index.tsv`，也是复核器逐图比对的依据）。
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub bytes: u64,
    pub bpp: f64,
    /// `None` = 无损（PSNR 无穷，TSV 里写 `inf`）。
    pub psnr: Option<f64>,
    pub max_err: u32,
    pub sha256: String,
}

pub fn tsv(entries: &[Entry]) -> String {
    let mut s = String::from("name\tbytes\tbpp\tpsnr\tmax_err\tsha256\n");
    for e in entries {
        let psnr = match e.psnr {
            Some(v) => format!("{v:.6}"),
            None => "inf".into(),
        };
        s.push_str(&format!(
            "{}\t{}\t{:.6}\t{}\t{}\t{}\n",
            e.name, e.bytes, e.bpp, psnr, e.max_err, e.sha256
        ));
    }
    s
}

pub fn parse_tsv(text: &str) -> Result<Vec<Entry>, String> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if i == 0 {
            if line != "name\tbytes\tbpp\tpsnr\tmax_err\tsha256" {
                return Err(format!("索引表头不对: {line}"));
            }
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() != 6 {
            return Err(format!("索引第 {} 行列数 {}: {line}", i + 1, f.len()));
        }
        let num = |s: &str, what: &str| -> Result<f64, String> {
            s.parse()
                .map_err(|_| format!("索引第 {} 行 {what} 非法: {s}", i + 1))
        };
        out.push(Entry {
            name: f[0].to_string(),
            bytes: num(f[1], "bytes")? as u64,
            bpp: num(f[2], "bpp")?,
            psnr: if f[3] == "inf" {
                None
            } else {
                Some(num(f[3], "psnr")?)
            },
            max_err: num(f[4], "max_err")? as u32,
            sha256: f[5].to_string(),
        });
    }
    Ok(out)
}

/// 编码：TTX2（可选 mip 链）。
pub fn encode(rgba: &[u8], width: usize, height: usize, mip: bool) -> Result<Vec<u8>, String> {
    if mip {
        crate::convert::rgba_to_ttx2_mips(rgba, width, height).map_err(|e| e.to_string())
    } else {
        crate::convert::rgba_to_ttx2(rgba, width, height).map_err(|e| e.to_string())
    }
}

/// 解码最细一级。
pub fn decode(ttx: &[u8]) -> Result<(usize, usize, Vec<u8>), String> {
    let img = crate::convert::ttx2_to_rgba(ttx).map_err(|e| e.to_string())?;
    Ok((img.width, img.height, img.rgba))
}

/// 逐图指标：`(psnr, max_err, sampling_checked, sampling_bad)`。
///
/// 采样一致性按**固定步长抽样**（默认 5 ⇒ 每图约 1/25 的纹素 + 四角），
/// 检查 nearest/bilinear 在纹素中心是否等于整图解码、repeat 是否跨边界回环。
pub fn measure(
    src: &[u8],
    width: usize,
    height: usize,
    ttx: &[u8],
    stride: usize,
) -> Result<(Option<f64>, u32, usize, usize), String> {
    let (w, h, dec) = decode(ttx)?;
    if (w, h) != (width, height) {
        return Err(format!("解码尺寸 {w}×{h} ≠ 源 {width}×{height}"));
    }
    let psnr = crate::psnr_rgb(src, &dec);
    let mut max_err = 0u32;
    for (a, b) in src.as_chunks::<4>().0.iter().zip(dec.as_chunks::<4>().0) {
        for c in 0..3 {
            max_err = max_err.max(a[c].abs_diff(b[c]) as u32);
        }
    }
    let doc = from_ttx2(ttx).map_err(|e| e.to_string())?;
    let levels = doc.levels().ok_or("不是纹理文档")?;
    let l0 = levels.first().ok_or("没有 mip 级")?;
    let mut clamp = Sampler::with_wrap(l0, Wrap::Clamp);
    let mut rep = Sampler::with_wrap(l0, Wrap::Repeat);
    let (mut checked, mut bad) = (0usize, 0usize);
    let st = stride.max(1);
    let mut ys: Vec<usize> = (0..h).step_by(st).collect();
    let mut xs: Vec<usize> = (0..w).step_by(st).collect();
    for y in [0, h - 1] {
        if !ys.contains(&y) {
            ys.push(y);
        }
    }
    for x in [0, w - 1] {
        if !xs.contains(&x) {
            xs.push(x);
        }
    }
    for &y in &ys {
        for &x in &xs {
            let want = &dec[(y * w + x) * 4..(y * w + x) * 4 + 4];
            let u = (x as f32 + 0.5) / w as f32;
            let v = (y as f32 + 0.5) / h as f32;
            checked += 3;
            if clamp.sample_nearest(u, v) != [want[0], want[1], want[2], want[3]] {
                bad += 1;
            }
            if clamp.sample_bilinear(u, v) != [want[0], want[1], want[2], want[3]] {
                bad += 1;
            }
            if rep.sample_nearest(u + 1.0, v) != [want[0], want[1], want[2], want[3]] {
                bad += 1;
            }
        }
    }
    Ok((psnr, max_err, checked, bad))
}

/// mip 链的尺寸不变量：每级不超过上一级的一半（向下取整，最小 1）。
pub fn mip_sizes(ttx: &[u8]) -> Result<Vec<(usize, usize)>, String> {
    let doc = from_ttx2(ttx).map_err(|e| e.to_string())?;
    let levels = doc.levels().ok_or("不是纹理文档")?;
    Ok(levels.iter().map(|t| (t.width, t.height)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tmp(PathBuf);

    impl Tmp {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("trit_texpack_{tag}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn checker(n: usize, a: u8, b: u8) -> Vec<u8> {
        let mut v = vec![0u8; n * n * 4];
        for y in 0..n {
            for x in 0..n {
                let c = if (x / 4 + y / 4) % 2 == 0 { a } else { b };
                let i = (y * n + x) * 4;
                v[i] = c;
                v[i + 1] = c;
                v[i + 2] = c;
                v[i + 3] = 255;
            }
        }
        v
    }

    #[test]
    fn fingerprint_is_content_and_name_sensitive() {
        // 指纹只看字节与文件名，不需要真 PNG（无 png 特性时这段也要跑）。
        let t = Tmp::new("fp");
        let p = t.0.join("a.png");
        std::fs::write(&p, b"same-bytes").unwrap();
        let h1 = fingerprint_pngs(&list_pngs(&t.0).unwrap()).unwrap();
        assert_eq!((h1.files, h1.bytes > 0), (1, true));
        // 同内容换名 → 指纹变（文件名也进规范序列）
        std::fs::rename(&p, t.0.join("b.png")).unwrap();
        assert_ne!(
            h1.hash,
            fingerprint_pngs(&list_pngs(&t.0).unwrap()).unwrap().hash
        );
        // 内容变化 → 指纹变
        std::fs::write(t.0.join("b.png"), b"other-bytes").unwrap();
        assert_ne!(
            h1.hash,
            fingerprint_pngs(&list_pngs(&t.0).unwrap()).unwrap().hash
        );
        // 非 png 扩展名不参与
        std::fs::write(t.0.join("c.txt"), b"x").unwrap();
        assert_eq!(list_pngs(&t.0).unwrap().len(), 1);
    }

    #[test]
    fn tsv_roundtrips() {
        let e = vec![
            Entry {
                name: "a.png".into(),
                bytes: 1024,
                bpp: 2.125,
                psnr: Some(33.25),
                max_err: 7,
                sha256: "ab".repeat(32),
            },
            Entry {
                name: "b.png".into(),
                bytes: 2048,
                bpp: 3.5,
                psnr: None,
                max_err: 0,
                sha256: "cd".repeat(32),
            },
        ];
        let text = tsv(&e);
        assert!(text.starts_with("name\tbytes\tbpp\tpsnr\tmax_err\tsha256\n"));
        assert_eq!(parse_tsv(&text).unwrap(), e);
        assert!(parse_tsv("bad\n").is_err());
        assert!(parse_tsv("name\tbytes\tbpp\tpsnr\tmax_err\tsha256\nx\t1\n").is_err());
    }

    #[test]
    fn encode_is_deterministic_and_metrics_are_sane() {
        let rgba = checker(32, 0, 255);
        let a = encode(&rgba, 32, 32, false).unwrap();
        let b = encode(&rgba, 32, 32, false).unwrap();
        assert_eq!(a, b);
        let (psnr, max_err, checked, bad) = measure(&rgba, 32, 32, &a, 4).unwrap();
        assert!(checked > 0);
        assert_eq!(bad, 0, "采样与解码不一致");
        assert!(psnr.is_none_or(|v| v > 20.0));
        assert!(max_err <= 255);
    }

    #[test]
    fn mip_sizes_halve() {
        let rgba = checker(32, 16, 240);
        let ttx = encode(&rgba, 32, 32, true).unwrap();
        let sizes = mip_sizes(&ttx).unwrap();
        assert_eq!(sizes[0], (32, 32));
        for w in sizes.windows(2) {
            assert!(w[1].0 <= (w[0].0 / 2).max(1) && w[1].1 <= (w[0].1 / 2).max(1));
        }
    }
}
