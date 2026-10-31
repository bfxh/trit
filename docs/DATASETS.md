# 训练素材清单（2026-10-31，已实测可访问性）

> 口径：**只有跑通过的调用才写「已验」**；需要注册/登录/人工审批的一律写「未验」，不照抄别人的说法。
> 本机磁盘：`D:` 23.7 GB 空闲、`E:` 4.4 GB（99% 满）、`C:` 15.2 GB。
> ⚠️ **E: 已接近满，数据集一律落 `D:/KF/trit-data/`**；大批量前先跑 `scripts/fetch_textures.sh` 的磁盘预检。

## A. 材质 / 纹理（N5 直接用）

| 站点 | 内容量 | 许可 | 访问方式（已验） | 备注 |
|---|---|---|---|---|
| **ambientCG** | **2,037 个 PBR 材质** | **CC0** | `GET https://ambientcg.com/api/v2/full_json?type=Material&limit=N&include=downloadData` → `foundAssets[i].downloadFolders...fullDownloadPath`，例 `https://ambientcg.com/get?file=Fabric086_1K-JPG.zip`（7.7 MB） | ✅ 已实测列出 + 拿到下载直链 |
| **Poly Haven** | **869 纹理 / 521 模型 / 997 HDRI** | **CC0** | `GET https://api.polyhaven.com/assets?t=textures|models|hdris`（**必须带 UA**，Python urllib 默认 UA 会 403） | ✅ 已实测计数；批量下载用 `files/<asset_id>` 拿文件表 |

## B. 3D 物体（S2/S3/S4 的几何与外观）

| 数据集 | 规模 | 许可 | 访问方式 | 状态 |
|---|---|---|---|---|
| **Objaverse-XL** | **10M+ 物体**（HF `size_categories: 10M<n<100M`） | ODC-By（逐个物体另有许可） | HF API 元数据已通：`GET https://huggingface.co/api/datasets/allenai/objaverse-xl`；实际对象需按官方脚本拉（HF 只存索引/清单） | ✅ 元数据已验；**对象级下载未验** |
| Objaverse 1.0 | 800K+ 带标注 | 同左 | `objaverse.allenai.org` | 未验 |
| Thingi10K | 1 万网格 | 逐模型（CC 系列） | GitHub `Thingi10K/Thingi10K` | 未验 |

## C. 真实场景 RGB-D（S5 现实域的前置）

| 数据集 | 规模 | 许可 | 状态 |
|---|---|---|---|
| ScanNet | 1,513 次扫描 / 707 场景 | 需注册同意（研究用途） | **未验**（本机无账号） |
| ARKitScenes | 5,048 序列 / 1,661 场景（LiDAR） | Apple 研究许可 | **未验** |
| Hypersim | 合成室内（77K 帧） | 研究用途 | **未验** |
| Replica | 18 个高质量场景 | 研究用途 | **未验** |

## D. 立刻能用的路线（已落地）

1. **ambientCG 子集**：按类别各取若干（fabric/wood/metal/stone/tiles），1K-JPG，约 8 MB/材质；
   `scripts/fetch_textures.sh` 已带：磁盘预检（<5 GB 停）、只留 Color 图、失败重试。
2. 转成 256² 灰度 PNG（`scripts/prep_textures.py`），喂 `parts-3d tex-prior --data` / `tex-gen --data`。
3. 规模账：全量 2,037 × 8 MB ≈ **16 GB**（D: 现空 23.7 GB，勉强够但会挤占其他数据）；
   建议先取 **200 张**（≈1.6 GB）作为 N5 训练集，验证收益后再决定是否扩到全量。

## E. 许可证与合规

- ambientCG / Poly Haven 均为 **CC0**，可商用、可再分发，无需署名（仍建议在数据清单里记录来源）。
- Objaverse-XL 整体 ODC-By，但**单个物体许可不一**（部分 CC-BY-NC/自定义）⇒ 只取允许商用的子集，
  并在清单里保存每个 object 的 `license` 字段。
- ScanNet/ARKitScenes/Hypersim/Replica 都是**研究用途 + 注册**，不能直接进商业管线。

## F. 实测：已接入并跑通（2026-10-31）

- **脚本**：`scripts/fetch_textures.py`（ambientCG 子集：带 UA、重试、只留 Color、磁盘 <5 GB 停）、
  `scripts/prep_textures.py`（转 256²/128² 灰度 PNG）。
  ⚠️ 同目录的 `fetch_textures.sh`（curl 版）**已弃用**：Git Bash 下 curl 对 Windows 路径/CRLF 组合
  会静默产出 0 字节（实测 0/8 全失败），改 Python 后 8/8、14/14 成功。
- **吞吐（实测）**：单线程约 12 个/分钟；**8 线程并发 149 个/约 2.5 分钟**（`JOBS` 参数）。
  单个材质只留 Color ≈ **1 MB**；**149 个 = 159 MB** 原始 + 8 MB PNG。
- **已训**（128² 灰度，147 训 / 2 留出）：3×3 三值 MLP 去噪 **29.04 dB**、7×7 **29.96 dB**
  （程序化同配置 24.82 dB；19 张真实数据时 3×3 只有 27.74 dB ⇒ **数据量 +1.3 dB，7×7 这时才反超**）。
- **全量账**：ambientCG 2,037 材质 × 1 MB(Color) ≈ 2 GB（不是 16 GB —— 之前按 zip 体积估高了）。
- `parts-3d tex-prior --data <png目录>` 已支持真实材质；`tex-gen` 的 `--data` 还没接（下一步）。
