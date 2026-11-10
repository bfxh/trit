//! S1 场景语料的**内容级**指纹（提取线 A1/A2 共用）。
//!
//! 覆盖场景重建真正读到的文件：`renders/<scene>/{seg,depth,meta}` 与你给的帧号，
//! 以及 `scenes/<scene>/objects.json`（GT 原语）。逐文件 SHA-256 后对
//! 「相对键 + 哈希」的规范序列再做一次总哈希——与深度/部件/纹理线同一口径。

use std::path::{Path, PathBuf};

use trit_core::fingerprint::{file_sha256, sha256_hex};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fp {
    pub dirs: usize,
    pub files: usize,
    pub bytes: u64,
    pub hash: String,
}

fn norm(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// `renders` / `scenes` 两个根 + 场景号 + 帧号 → 内容级指纹。
///
/// 场景号排序去重；`objects.json` 缺失即报错（训练/评测的 GT 不能悄悄缺）。
pub fn fingerprint_scenes(
    renders: &Path,
    scenes: &Path,
    scene_ids: &[usize],
    frames: &[usize],
) -> Result<Fp, String> {
    let mut ids: Vec<usize> = scene_ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Err("场景列表为空".into());
    }
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    for &idx in &ids {
        let name = format!("scene{idx:04}");
        let rd = renders.join(&name);
        for &f in frames {
            for (sub, ext) in [("seg", "npy"), ("depth", "npy"), ("meta", "json")] {
                let p = rd.join(sub).join(format!("{f:06}.{ext}"));
                files.push((format!("{name}/{sub}/{f:06}"), p));
            }
        }
        files.push((
            format!("{name}/objects.json"),
            scenes.join(&name).join("objects.json"),
        ));
    }
    let mut bytes = 0u64;
    let mut lines = Vec::with_capacity(files.len());
    for (rel, p) in &files {
        let meta = std::fs::metadata(p).map_err(|e| format!("{}: {e}", p.display()))?;
        bytes += meta.len();
        lines.push(format!("{rel} {}", file_sha256(p)?));
    }
    Ok(Fp {
        dirs: ids.len(),
        files: files.len(),
        bytes,
        hash: sha256_hex(lines.join("\n").as_bytes()),
    })
}

/// 单场景 + 帧集合的内容级指纹（资产导出/复核共用；只覆盖模型真正读的 seg/depth/meta）。
pub fn fingerprint_scene_frames(scene: &Path, frames: &[usize]) -> Result<Fp, String> {
    scene_frames(
        scene,
        frames,
        &[("seg", "npy"), ("depth", "npy"), ("meta", "json")],
    )
}

/// 同上并覆盖 `rgb/`（高斯导出要用 RGB 算 PSNR）。
pub fn fingerprint_scene_frames_rgb(scene: &Path, frames: &[usize]) -> Result<Fp, String> {
    scene_frames(
        scene,
        frames,
        &[
            ("seg", "npy"),
            ("depth", "npy"),
            ("meta", "json"),
            ("rgb", "png"),
        ],
    )
}

fn scene_frames(scene: &Path, frames: &[usize], subs: &[(&str, &str)]) -> Result<Fp, String> {
    let mut lines = Vec::new();
    let mut bytes = 0u64;
    let mut files = 0usize;
    for &f in frames {
        for (sub, ext) in subs {
            let p = scene.join(sub).join(format!("{f:06}.{ext}"));
            let meta = std::fs::metadata(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            bytes += meta.len();
            files += 1;
            lines.push(format!("{sub}/{f:06} {}", file_sha256(&p)?));
        }
    }
    Ok(Fp {
        dirs: 1,
        files,
        bytes,
        hash: sha256_hex(
            lines
                .join(
                    "
",
                )
                .as_bytes(),
        ),
    })
}

/// 场景根的规范写法（清单里直接记这个，复核器照抄）。
pub fn norm_root(p: &Path) -> String {
    norm(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tmp(PathBuf);

    impl Tmp {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("trit_scenefp_{tag}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
        fn put(&self, rel: &str, bytes: &[u8]) {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, bytes).unwrap();
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scene(t: &Tmp, idx: usize, frames: &[usize]) {
        for f in frames {
            t.put(&format!("r/scene{idx:04}/seg/{f:06}.npy"), b"seg");
            t.put(&format!("r/scene{idx:04}/depth/{f:06}.npy"), b"dep");
            t.put(&format!("r/scene{idx:04}/meta/{f:06}.json"), b"{}");
        }
        t.put(&format!("s/scene{idx:04}/objects.json"), b"[]");
    }

    #[test]
    fn fingerprint_is_order_insensitive_but_content_sensitive() {
        let t = Tmp::new("a");
        scene(&t, 80, &[0, 1]);
        let a = fingerprint_scenes(&t.0.join("r"), &t.0.join("s"), &[80, 81], &[0, 1]);
        // 81 不存在 ⇒ 报错（不许悄悄少场景）
        assert!(a.is_err());
        scene(&t, 81, &[0, 1]);
        let a = fingerprint_scenes(&t.0.join("r"), &t.0.join("s"), &[80, 81], &[0, 1]).unwrap();
        let b = fingerprint_scenes(&t.0.join("r"), &t.0.join("s"), &[81, 80, 81], &[0, 1]).unwrap();
        assert_eq!(a, b, "场景顺序/重复不应改变指纹");
        // 相机 meta 变化必须换指纹（occ 的输入特征依赖它）
        t.put("r/scene0080/meta/000000.json", b"{\"camera\":1}");
        let c = fingerprint_scenes(&t.0.join("r"), &t.0.join("s"), &[80, 81], &[0, 1]).unwrap();
        assert_ne!(a.hash, c.hash);
        // 2 场景 × (2 帧 × {seg,depth,meta} + objects.json) = 14
        assert_eq!(a.files, 14);
    }

    #[test]
    fn single_scene_fingerprint_matches_frames() {
        let t = Tmp::new("one");
        scene(&t, 80, &[0, 1]);
        let a = fingerprint_scene_frames(&t.0.join("r/scene0080"), &[0]).unwrap();
        let b = fingerprint_scene_frames(&t.0.join("r/scene0080"), &[0, 1]).unwrap();
        assert_eq!(a.files, 3);
        assert_eq!(b.files, 6);
        assert_ne!(a.hash, b.hash);
        assert!(fingerprint_scene_frames(&t.0.join("r/scene0080"), &[9]).is_err());
    }

    #[test]
    fn rgb_variant_covers_one_more_file_per_frame() {
        let t = Tmp::new("rgb");
        scene(&t, 80, &[0, 1]);
        t.put("r/scene0080/rgb/000000.png", b"png0");
        t.put("r/scene0080/rgb/000001.png", b"png1");
        let a = fingerprint_scene_frames(&t.0.join("r/scene0080"), &[0, 1]).unwrap();
        let b = fingerprint_scene_frames_rgb(&t.0.join("r/scene0080"), &[0, 1]).unwrap();
        assert_eq!((a.files, b.files), (6, 8));
        assert_ne!(a.hash, b.hash);
        let c = fingerprint_scene_frames_rgb(&t.0.join("r/scene0080"), &[0, 1]).unwrap();
        t.put("r/scene0080/rgb/000000.png", b"pngX");
        let d = fingerprint_scene_frames_rgb(&t.0.join("r/scene0080"), &[0, 1]).unwrap();
        assert_eq!(b, c);
        assert_ne!(b.hash, d.hash, "RGB 变化必须换指纹");
    }

    #[test]
    fn frames_are_part_of_the_fingerprint() {
        let t = Tmp::new("frames");
        scene(&t, 80, &[0, 1]);
        let a = fingerprint_scenes(&t.0.join("r"), &t.0.join("s"), &[80], &[0]).unwrap();
        let b = fingerprint_scenes(&t.0.join("r"), &t.0.join("s"), &[80], &[0, 1]).unwrap();
        assert_ne!(a.hash, b.hash);
        assert_eq!(a.files, 4);
        assert_eq!(b.files, 7);
    }
}
