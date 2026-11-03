//! 部件线（A1/A2）共用的**数据枚举 + 内容级指纹**。
//!
//! 与深度线同一口径（`depthdata::data_fingerprint`）：逐文件 SHA-256，再对
//! 「相对路径 + 文件哈希」的规范序列做一次总哈希。训练清单与独立复核器都调这里，
//! 保证两边枚举的是同一批文件（不是各自抄一份名单）。

use std::path::{Path, PathBuf};

use crate::fingerprint::{file_sha256, sha256_hex};

/// 一份数据集的内容级指纹。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fp {
    pub dirs: usize,
    pub files: usize,
    pub bytes: u64,
    pub hash: String,
}

/// 路径写法归一：`E:\x` 与 `E:/x` 必须得到同一指纹。
pub fn norm(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// `--data` 的展开规则（与 parts-train 历史行为一致）：
/// 目录含 `rgb/` 就是场景本身，否则取其下含 `rgb/` 的子目录（排序）。
pub fn expand_scenes(root: &Path) -> Vec<PathBuf> {
    if root.join("rgb").is_dir() {
        return vec![root.to_path_buf()];
    }
    let mut subs: Vec<PathBuf> = std::fs::read_dir(root)
        .map(|it| {
            it.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.join("rgb").is_dir())
                .collect()
        })
        .unwrap_or_default();
    subs.sort();
    subs
}

/// 训练用的 `(场景, 帧 png)` 列表；`limit` 按**全局帧数**截断（与 parts-train 同口径）。
pub fn expand_frames(roots: &[PathBuf], limit: usize) -> Vec<(PathBuf, PathBuf)> {
    let mut v = Vec::new();
    for root in roots {
        for scene in expand_scenes(root) {
            for p in crate::data::frames(&scene) {
                v.push((scene.clone(), p));
            }
        }
    }
    if limit > 0 {
        v.truncate(limit);
    }
    v
}

/// `parts-inst` 的场景枚举：`renders/` 下以 `scene` 或 `pt` 开头的子目录。
pub fn scene_dirs(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(root)
        .map_err(|e| format!("读不了 {}: {e}", root.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.starts_with("scene") || s.starts_with("pt"))
        })
        .collect();
    v.sort();
    Ok(v)
}

/// `parts-inst` 的 GT 帧枚举：`<renders>/<scene>/seg/*.npy`（排序）。
pub fn seg_files(rd: &Path) -> Result<Vec<PathBuf>, String> {
    let dir = rd.join("seg");
    let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|e| format!("读不了 {}: {e}", dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "npy"))
        .collect();
    v.sort();
    Ok(v)
}

struct Item {
    key: String,
    path: PathBuf,
}

fn hash_items(items: Vec<Item>, dirs: usize) -> Result<Fp, String> {
    let mut bytes = 0u64;
    for it in &items {
        bytes += std::fs::metadata(&it.path)
            .map_err(|e| format!("{}: {e}", it.path.display()))?
            .len();
    }
    let nthreads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 8);
    let chunk = items.len().div_ceil(nthreads).max(1);
    let mut parts: Vec<Vec<String>> = Vec::new();
    let joined: Result<(), String> = std::thread::scope(|s| {
        let mut handles = Vec::new();
        for c in items.chunks(chunk) {
            handles.push(s.spawn(move || -> Result<Vec<String>, String> {
                let mut out = Vec::with_capacity(c.len());
                for it in c {
                    let h = file_sha256(&it.path)?;
                    out.push(format!("{} {h}", it.key));
                }
                Ok(out)
            }));
        }
        for h in handles {
            let got = h.join().map_err(|_| "指纹线程 panic".to_string())?;
            parts.push(got?);
        }
        Ok(())
    });
    joined?;
    // 线程数变化不影响结果：合并顺序 = items 的原顺序。
    let mut acc: Vec<u8> = Vec::new();
    for line in parts.iter().flatten() {
        acc.extend_from_slice(line.as_bytes());
        acc.push(b'\n');
    }
    Ok(Fp {
        dirs,
        files: items.len(),
        bytes,
        hash: sha256_hex(&acc),
    })
}

/// 训练集指纹：每帧的 `rgb/<stem>.png` + `<seg_sub>/<stem>.npy`。
pub fn fingerprint_train(frames: &[(PathBuf, PathBuf)], seg_sub: &str) -> Result<Fp, String> {
    let mut items = Vec::with_capacity(frames.len() * 2);
    let mut scenes: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for (scene, rgb) in frames {
        let stem = rgb
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| format!("文件名非法: {}", rgb.display()))?;
        let s = norm(scene);
        scenes.insert(s.clone());
        items.push(Item {
            key: format!("{s}/rgb/{stem}.png"),
            path: rgb.clone(),
        });
        let lab = scene.join(seg_sub).join(format!("{stem}.npy"));
        if !lab.is_file() {
            return Err(format!("缺标签 {}", lab.display()));
        }
        items.push(Item {
            key: format!("{s}/{seg_sub}/{stem}.npy"),
            path: lab,
        });
    }
    hash_items(items, scenes.len())
}

/// 评测语料指纹：与 `parts-inst eval` 读的同一批文件
/// （`scenes/<s>/instances.json`、`renders/<s>/rgb|seg/*`，可选的 `bnd`/`pbnd`）。
pub fn fingerprint_eval(renders: &Path, scenes: &Path, limit: usize) -> Result<Fp, String> {
    let sds = scene_dirs(scenes)?;
    let mut items = Vec::new();
    for sd in &sds {
        let name = sd
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| format!("场景名非法: {}", sd.display()))?;
        let inst = sd.join("instances.json");
        if inst.is_file() {
            items.push(Item {
                key: format!("scenes/{name}/instances.json"),
                path: inst,
            });
        }
        let rd = renders.join(name);
        for (i, f) in seg_files(&rd)?.iter().enumerate() {
            if limit > 0 && i >= limit {
                break;
            }
            let stem = f
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| format!("文件名非法: {}", f.display()))?;
            items.push(Item {
                key: format!("renders/{name}/rgb/{stem}.png"),
                path: rd.join("rgb").join(format!("{stem}.png")),
            });
            items.push(Item {
                key: format!("renders/{name}/seg/{stem}.npy"),
                path: f.clone(),
            });
            for sub in ["bnd", "pbnd"] {
                let p = rd.join(sub).join(format!("{stem}.npy"));
                if p.is_file() {
                    items.push(Item {
                        key: format!("renders/{name}/{sub}/{stem}.npy"),
                        path: p,
                    });
                }
            }
        }
    }
    hash_items(items, sds.len())
}

/// 清单里的 JSON 字符串转义（不引 serde）。
pub fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// `git <args>` 的 stdout（失败给空串，清单里不因此中断训练）。
pub fn git_out(args: &[&str]) -> String {
    std::process::Command::new("git")
        .args(args)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

pub fn created_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tmp(PathBuf);

    impl Tmp {
        fn new(tag: &str) -> Self {
            let p =
                std::env::temp_dir().join(format!("trit_partsrun_{tag}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
        fn write(&self, rel: &str, bytes: &[u8]) -> PathBuf {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, bytes).unwrap();
            p
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn expand_frames_respects_roots_and_limit() {
        let t = Tmp::new("expand");
        for s in ["scene0000", "scene0001"] {
            t.write(&format!("{s}/rgb/000000.png"), b"a");
            t.write(&format!("{s}/rgb/000001.png"), b"b");
        }
        let roots = vec![t.0.clone()];
        assert_eq!(expand_frames(&roots, 0).len(), 4);
        assert_eq!(expand_frames(&roots, 3).len(), 3);
        // 单场景根（自身含 rgb/）
        assert_eq!(expand_frames(&[t.0.join("scene0000")], 0).len(), 2);
    }

    #[test]
    fn train_fingerprint_is_content_sensitive() {
        let t = Tmp::new("train");
        t.write("s/rgb/000000.png", b"rgb-a");
        t.write("s/seg/000000.npy", b"lab-a");
        let frames = vec![(t.0.join("s"), t.0.join("s/rgb/000000.png"))];
        let a = fingerprint_train(&frames, "seg").unwrap();
        assert_eq!((a.dirs, a.files), (1, 2));
        t.write("s/seg/000000.npy", b"lab-b");
        let b = fingerprint_train(&frames, "seg").unwrap();
        assert_ne!(a.hash, b.hash);
        assert_eq!(a.files, b.files);
    }

    #[test]
    fn train_fingerprint_distinguishes_paths() {
        // 同内容换目录名也必须换指纹（否则「把数据换个目录」就能骗过复核）
        let t = Tmp::new("paths");
        t.write("a/rgb/000000.png", b"same");
        t.write("a/seg/000000.npy", b"same");
        t.write("b/rgb/000000.png", b"same");
        t.write("b/seg/000000.npy", b"same");
        let fa = vec![(t.0.join("a"), t.0.join("a/rgb/000000.png"))];
        let fb = vec![(t.0.join("b"), t.0.join("b/rgb/000000.png"))];
        assert_ne!(
            fingerprint_train(&fa, "seg").unwrap().hash,
            fingerprint_train(&fb, "seg").unwrap().hash
        );
    }

    #[test]
    fn train_fingerprint_rejects_missing_label() {
        let t = Tmp::new("nolab");
        t.write("s/rgb/000000.png", b"x");
        let frames = vec![(t.0.join("s"), t.0.join("s/rgb/000000.png"))];
        assert!(fingerprint_train(&frames, "seg").is_err());
    }

    #[test]
    fn eval_fingerprint_enumerates_used_files_and_limit() {
        let t = Tmp::new("eval");
        for fr in 0..3 {
            t.write(&format!("renders/scene0000/rgb/{fr:06}.png"), b"r");
            t.write(&format!("renders/scene0000/seg/{fr:06}.npy"), b"s");
        }
        t.write("renders/scene0000/pbnd/000000.npy", b"b");
        t.write("scenes/scene0000/instances.json", b"{}");
        let renders = t.0.join("renders");
        let scenes = t.0.join("scenes");
        let all = fingerprint_eval(&renders, &scenes, 0).unwrap();
        // 3 帧 × (rgb + seg) + pbnd + instances
        assert_eq!(all.files, 8);
        assert_eq!(all.dirs, 1);
        let two = fingerprint_eval(&renders, &scenes, 2).unwrap();
        assert_eq!(two.files, 2 * 2 + 1 + 1);
        // 内容变化必须换指纹
        t.write("renders/scene0000/seg/000000.npy", b"changed");
        assert_ne!(
            all.hash,
            fingerprint_eval(&renders, &scenes, 0).unwrap().hash
        );
    }

    #[test]
    fn eval_fingerprint_rejects_missing_rgb() {
        let t = Tmp::new("norqgb");
        t.write("renders/scene0000/seg/000000.npy", b"s");
        t.write("scenes/scene0000/instances.json", b"{}");
        assert!(fingerprint_eval(&t.0.join("renders"), &t.0.join("scenes"), 0).is_err());
    }

    #[test]
    fn json_and_git_helpers_are_total() {
        assert_eq!(json_str("a\"b\\c\n"), "\"a\\\"b\\\\c\\n\"");
        // 非法子命令不应 panic
        let _ = git_out(&["rev-parse", "--definitely-not-a-flag"]);
    }
}
