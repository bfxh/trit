//! 深度头训练/评测共享的样本加载。
//!
//! CPU（`parts-depth cnn-train`）与 GPU（`parts-depth-gpu`）两条训练线必须用**同一份**
//! 预处理：中心裁剪/整图降采样、前景判定、逐图 log 深度归一化的统计口径都在这里。

use std::path::{Path, PathBuf};

use crate::data::read_rgb;

/// 一条样本：RGB 平面 / **原始 log 深度** / 有效掩码 / 逐图中位与四分位距 / 原图宽。
///
/// 归一化统计是**数据集级**的（存进 `DepthNet::med/iqr`）：逐图归一化会把绝对尺度抹掉，
/// 推理时拿不到「这张图自己的中位数」，网格就没有尺度。
pub type DepthSample = (Vec<Vec<f32>>, Vec<f32>, Vec<bool>, f32, f32, usize);

/// 采样窗口是否落在图内。`size` 来自 `--size`：`0` 除零、超过图宽高会让 `w-size` 下溢；判空返回不 panic。
pub fn window_is_usable(size: usize, w: usize, h: usize) -> bool {
    size > 0 && size <= w && size <= h
}

/// 读一张图 + 深度，返回 `(rgb 平面, 原始 log 深度, 掩码)`。
///
/// `full=true` 时**整图盒式降采样**到 `size²`（前景只占 ~15%，中心裁剪会丢掉全局布局；
/// 64² 整图是 6×6 块均值）；否则中心裁剪。
pub fn load_sample(
    scene: &Path,
    frame: usize,
    size: usize,
    full: bool,
    pyr: usize,
    ppyr: usize,
) -> Option<DepthSample> {
    if !window_is_usable(size, usize::MAX, usize::MAX) {
        return None; // size == 0：下面 `w / size` 会除零，连图都不必读
    }
    let p = scene.join("rgb").join(format!("{frame:06}.png"));
    if !p.exists() {
        return None;
    }
    let (w, h, rgb) = read_rgb(&p).ok()?;
    if !window_is_usable(size, w, h) {
        return None;
    }
    let (dep, _) =
        crate::geo::read_npy_f32(&scene.join("depth").join(format!("{frame:06}.npy"))).ok()?;
    let (seg, _) =
        crate::data::read_npy_i32(&scene.join("seg").join(format!("{frame:06}.npy"))).ok()?;
    // depth/seg 必须覆盖全部像素（口径同 `data::depth_planes`）；短数组按 `si` 索引会越界 panic，判空返回。
    if dep.len() < w * h || seg.len() < w * h {
        return None;
    }
    // 已证明 `size` 落在图内，`cells` 至多 `w * h`；三处缓冲共用同一份，省掉重复裸乘。
    let cells = size * size;
    let (x0, y0) = ((w - size) / 2, (h - size) / 2);
    let mut planes = vec![vec![0.0f32; cells]; 3];
    let mut disp = vec![0.0f32; cells];
    let mut mask = vec![false; cells];
    let mut vals: Vec<f32> = Vec::new();
    for y in 0..size {
        for x in 0..size {
            let mut logd_sum = 0.0f32;
            let mut logd_n = 0usize;
            let mut rgb_sum = [0.0f32; 3];
            let mut rgb_n = 0usize;
            if full {
                let (sx, sy) = (w / size, h / size);
                for by in 0..sy {
                    for bx in 0..sx {
                        let si = (y * sy + by) * w + x * sx + bx;
                        for c in 0..3 {
                            rgb_sum[c] += rgb[si * 3 + c];
                        }
                        rgb_n += 1;
                        let d = dep[si];
                        if d.is_finite() && d > 0.05 && d < 50.0 && seg[si] > 0 {
                            logd_sum += d.ln();
                            logd_n += 1;
                        }
                    }
                }
                for c in 0..3 {
                    planes[c][y * size + x] = (rgb_sum[c] / rgb_n.max(1) as f32 - 0.5) * 2.0;
                }
                // 前景占比 <25% 的块不参与训练（块里多为背景时均值没有意义）
                if logd_n * 4 >= rgb_n {
                    let dv = logd_sum / logd_n as f32;
                    disp[y * size + x] = dv;
                    mask[y * size + x] = true;
                    vals.push(dv);
                }
            } else {
                let si = (y0 + y) * w + x0 + x;
                for c in 0..3 {
                    // 输入标准化到约 [-1,1]：原值 0..1 在小 lr 下推不动
                    planes[c][y * size + x] = (rgb[si * 3 + c] - 0.5) * 2.0;
                }
                let d = dep[si];
                // 目标是 **log 深度**（比视差稳：近处不会放大成几百倍）
                if d.is_finite() && d > 0.05 && d < 50.0 && seg[si] > 0 {
                    let dv = d.ln();
                    disp[y * size + x] = dv;
                    mask[y * size + x] = true;
                    vals.push(dv);
                }
            }
        }
    }
    if vals.len() < cells / 20 {
        return None;
    }
    let (med, iqr) = {
        let mut v = vals.clone();
        v.sort_by(f32::total_cmp);
        let q = |f: f32| v[((v.len() as f32 - 1.0) * f).round() as usize];
        (q(0.5), (q(0.75) - q(0.25)).max(1e-3))
    };
    for lvl in 1..=pyr {
        add_lowpass(&mut planes, size, 4usize.pow(lvl as u32));
    }
    add_pyramid_pool(&mut planes, size, ppyr);
    Some((planes, disp, mask, med, iqr, w))
}

/// 多尺度输入：把每通道按 `factor` 做块均值再复制回原分辨率（`--pyr`）。
pub fn add_lowpass(planes: &mut Vec<Vec<f32>>, size: usize, factor: usize) {
    // `factor` 是调用方的 `4^lvl`（`--pyr` 从 argv 来）：回绕成 0 时 `size / factor` 除零，
    // 比网格还大时 `s = size / factor` 成为 0、`coarse[...]` 立刻越界。这一级没有分辨率
    // 可降，按 no-op 处理，与 `levels == 0` 时 `add_pyramid_pool` 直接返回同一口径。
    if factor == 0 || size < factor {
        return;
    }
    let n = planes.len();
    let s = size / factor;
    for c in 0..n {
        let src = &planes[c];
        let mut coarse = vec![0.0f32; s * s];
        for y in 0..s {
            for x in 0..s {
                let mut acc = 0.0f32;
                for by in 0..factor {
                    for bx in 0..factor {
                        acc += src[(y * factor + by) * size + x * factor + bx];
                    }
                }
                coarse[y * s + x] = acc / (factor * factor) as f32;
            }
        }
        let mut out = vec![0.0f32; size * size];
        for y in 0..size {
            for x in 0..size {
                out[y * size + x] = coarse[(y / factor) * s + x / factor];
            }
        }
        planes.push(out);
    }
}

/// **全局**金字塔池化输入：按 `2^lvl × 2^lvl` 网格取每通道块均值，再铺回全分辨率。
///
/// 与 `add_lowpass`（局部低通）不同，这里带的是整幅布局（1×1、2×2、4×4 …），
/// 每个 level 追加 3 个平面（RGB 各一）。
pub fn add_pyramid_pool(planes: &mut Vec<Vec<f32>>, size: usize, levels: usize) {
    if levels == 0 {
        return;
    }
    let base: Vec<Vec<f32>> = planes[..3].to_vec();
    for lvl in 1..=levels {
        let cells = 1usize << lvl; // 2, 4, 8 ...
        if cells > size {
            break;
        }
        let block = size / cells;
        for src in base.iter() {
            let mut out = vec![0.0f32; size * size];
            for cy in 0..cells {
                for cx in 0..cells {
                    let mut acc = 0.0f32;
                    for by in 0..block {
                        for bx in 0..block {
                            acc += src[(cy * block + by) * size + cx * block + bx];
                        }
                    }
                    let v = acc / (block * block) as f32;
                    for by in 0..block {
                        for bx in 0..block {
                            out[(cy * block + by) * size + cx * block + bx] = v;
                        }
                    }
                }
            }
            planes.push(out);
        }
    }
}

/// 一条训练样本：多视角一致性需要**相机**与逐图归一化统计。
#[derive(Clone)]
pub struct TrainSample {
    pub scene: PathBuf,
    pub scene_id: usize,
    pub frame: usize,
    pub x: Vec<Vec<f32>>,
    pub y: Vec<f32>,
    pub mask: Vec<bool>,
    pub med: f32,
    pub iqr: f32,
    pub cam: crate::geo::Camera,
}

/// 把 384² 的相机缩到模型网格（像素坐标按 `scale` 缩放）。
pub fn scaled_camera(scene: &Path, frame: usize, scale: f64) -> crate::geo::Camera {
    let p = scene.join("meta").join(format!("{frame:06}.json"));
    let mut c = crate::geo::Camera::load(&p).unwrap_or_else(|e| panic!("{e}"));
    for row in c.k.iter_mut().take(2) {
        for value in row.iter_mut().take(3) {
            *value *= scale;
        }
    }
    c
}

/// `--holdout` 可能给目录名也可能给完整路径（Git Bash 下 basename 不一定剥掉盘符）
/// ⇒ 一律取最后一段再比。
///
/// 这条规则来自 §6.90 的部署级 bug：`basename` 对 `E:/…` 不剥盘符，留出 0 命中，
/// 于是「留出」被当成训练集，读数全部作废。
pub fn is_held(path: &Path, holdout: &[String]) -> bool {
    let Some(name) = path.file_name().map(|s| s.to_string_lossy().into_owned()) else {
        return false;
    };
    holdout.iter().any(|h| {
        let hb = h.trim_end_matches(['/', '\\']);
        let hb = hb.rsplit(['/', '\\']).next().unwrap_or(hb);
        name == hb
    })
}

/// 评测用样本（只需要输入/目标/掩码；不带相机，验证器不必读 meta）。
pub struct EvalSample {
    pub x: Vec<Vec<f32>>,
    pub y: Vec<f32>,
    pub mask: Vec<bool>,
}

/// 留出集评测读数（与 `cnn-train` 尾段同口径）。
pub struct EvalReport {
    pub frames: usize,
    pub pixels: usize,
    pub mae_log: f64,
    pub rel_pct: f64,
    pub base_pct: f64,
    pub delta: f64,
    pub deploy_pct: f64,
}

/// 在留出样本上评 `DepthNet`：形状对齐 / 常数基线 / δ<1.25 / 默认尺度。
///
/// 留出样本的 `y` 必须是**原始 log 深度**（未做逐图归一化）。
pub fn eval_holdout(
    net: &crate::depthnet::DepthNet,
    held: &[EvalSample],
    size: usize,
) -> EvalReport {
    eval_with(|x| net.forward(x, size, size), net.med, net.iqr, held, size)
}

/// 用任意前向函数评留出集（U-Net 等其它头复用同一份口径）。
pub fn eval_with<F: Fn(&[Vec<f32>]) -> Vec<f32>>(
    forward: F,
    med: f32,
    iqr: f32,
    held: &[EvalSample],
    size: usize,
) -> EvalReport {
    use crate::depthnet::DepthNet;
    let _ = std::marker::PhantomData::<DepthNet>;
    let (mut mae_n, mut rel, mut rel_dep, mut rel_base, mut delta, mut n) =
        (0f64, 0f64, 0f64, 0f64, 0f64, 0usize);
    for s in held {
        let (x, y, mk) = (&s.x, &s.y, &s.mask);
        let mut v: Vec<f32> = y
            .iter()
            .zip(mk)
            .filter(|(_, m)| **m)
            .map(|(a, _)| *a)
            .collect();
        if v.is_empty() {
            continue;
        }
        v.sort_by(f32::total_cmp);
        let q = |f: f32| v[((v.len() as f32 - 1.0) * f).round() as usize];
        let (gmed, giqr) = (q(0.5), (q(0.75) - q(0.25)).max(1e-3));
        let p = forward(x);
        for i in 0..size * size {
            if !mk[i] {
                continue;
            }
            let raw = y[i];
            let aligned = p[i] * giqr + gmed;
            let dep = p[i] * iqr + med;
            let e = f64::from((aligned - raw).abs());
            mae_n += e;
            rel += (e.exp() - 1.0).abs();
            rel_dep += (f64::from(dep - raw).exp() - 1.0).abs();
            rel_base += (f64::from(raw - gmed).exp() - 1.0).abs();
            delta += f64::from(u8::from(e < 0.2231));
            n += 1;
        }
    }
    let k = n.max(1) as f64;
    EvalReport {
        frames: held.len(),
        pixels: n,
        mae_log: mae_n / k,
        rel_pct: rel / k * 100.0,
        base_pct: rel_base / k * 100.0,
        delta: delta / k,
        deploy_pct: rel_dep / k * 100.0,
    }
}

/// 数据集指纹（**内容级**）：对训练实际会用到的帧（`0..=24` 按 `stride`）逐文件流式
/// SHA-256，再对「(场景名, 子目录, 帧号, 文件哈希)」的规范序列做一次总哈希。
///
/// 覆盖 `rgb/`、`depth/`、`seg/` 三个子目录（训练读的就是这三类）；`meta/` 不参与
/// （本训练器不用相机）。结果对目录名与文件内容都敏感，同尺寸同帧数的内容替换也能抓到。
///
/// 返回 `(目录数, 文件数, 总字节数, sha256)`。代价：按实际数据读一遍（本机 ~2 GB 约 6 s）。
pub fn data_fingerprint(dirs: &[std::path::PathBuf], stride: usize) -> (usize, usize, u64, String) {
    use crate::fingerprint::{file_sha256, sha256_hex};
    // 路径写法归一（`E:\x` 与 `E:/x` 必须得到同一指纹），再排序。
    let mut names: Vec<String> = dirs
        .iter()
        .map(|d| d.to_string_lossy().replace('\\', "/"))
        .collect();
    names.sort();

    // 按规范顺序列出条目（目录名一行，随后每个文件的「子目录/帧号」标记），
    // 文件内容由线程池并行哈希；合并时按原顺序拼接 ⇒ 结果与线程数/调度无关。
    enum Item {
        Dir(String),
        File(String, std::path::PathBuf),
    }
    let mut items: Vec<Item> = Vec::new();
    let mut bytes = 0u64;
    let mut files = 0usize;
    for d in &names {
        items.push(Item::Dir(d.clone()));
        for fr in (0..=24).step_by(stride.max(1)) {
            for (sub, ext) in [("rgb", "png"), ("depth", "npy"), ("seg", "npy")] {
                let p = std::path::Path::new(d)
                    .join(sub)
                    .join(format!("{fr:06}.{ext}"));
                if let Ok(meta) = std::fs::metadata(&p) {
                    bytes += meta.len();
                    files += 1;
                    items.push(Item::File(format!("{sub}/{fr:06}"), p));
                }
            }
        }
    }

    let nthreads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 8);
    let chunk = items.len().div_ceil(nthreads).max(1);
    let mut parts: Vec<Vec<String>> = Vec::new();
    std::thread::scope(|s| {
        let mut handles = Vec::new();
        for c in items.chunks(chunk) {
            handles.push(s.spawn(move || {
                c.iter()
                    .map(|it| match it {
                        Item::Dir(d) => d.clone(),
                        Item::File(tag, p) => match file_sha256(p) {
                            Ok(h) => format!("{tag} {h}"),
                            Err(_) => format!("{tag} unreadable"),
                        },
                    })
                    .collect::<Vec<String>>()
            }));
        }
        for h in handles {
            parts.push(h.join().unwrap_or_default());
        }
    });

    let mut acc: Vec<u8> = Vec::new();
    for line in parts.iter().flatten() {
        acc.extend_from_slice(line.as_bytes());
        acc.push(b'\n');
    }
    (names.len(), files, bytes, sha256_hex(&acc))
}

#[cfg(test)]
mod tests;
