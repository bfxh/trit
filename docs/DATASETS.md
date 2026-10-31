# 训练素材清单（2026-11-01，已实测可访问性）

> 口径：**只有跑通过的调用才写「已验」**；需要注册/登录/人工审批的一律写「未验」，不照抄别人的说法。
> 本机磁盘：`D:` 23.7 GB 空闲、`E:` 4.4 GB（99% 满）、`C:` 15.2 GB。
> ⚠️ **E: 已接近满，数据集一律落 `D:/KF/trit-data/`**；大批量前先看 `scripts/fetch_textures.py` 的磁盘预检（<5 GB 停）。

## A. 材质 / 纹理（N5 直接用）

| 站点 | 内容量 | 许可 | 访问方式（已验） | 备注 |
|---|---|---|---|---|
| **ambientCG** | **2,037 个 PBR 材质** | **CC0** | `GET https://ambientcg.com/api/v2/full_json?type=Material&limit=N&include=downloadData` → `foundAssets[i].downloadFolders...fullDownloadPath`，例 `https://ambientcg.com/get?file=Fabric086_1K-JPG.zip`（7.7 MB） | ✅ 已实测列出 + 拿到下载直链 |
| **Poly Haven** | **869 纹理 / 521 模型 / 997 HDRI** | **CC0** | `GET https://api.polyhaven.com/assets?t=textures|models|hdris`（**必须带 UA**，Python urllib 默认 UA 会 403） | ✅ 已实测计数；批量下载用 `files/<asset_id>` 拿文件表 |

## B. 3D 物体（S2/S3/S4 的几何与外观）

| 数据集 | 规模 | 许可 | 访问方式 | 状态 |
|---|---|---|---|---|
| **Objaverse-XL** | **10M+ 物体**（HF `size_categories: 10M<n<100M`） | ODC-By（逐个物体另有许可） | HF API 元数据已通：`GET https://huggingface.co/api/datasets/allenai/objaverse-xl`；实际对象需按官方脚本拉（HF 只存索引/清单） | ✅ 元数据已验；**对象级下载未验** |
| Objaverse 1.0 | 800K+ 带标注 | 逐物体（metadata 里有 `license`） | `scripts/fetch_objaverse.py`：`metadata/<shard>.json.gz` + `glbs/<shard>/<uid>.glb`（HF resolve 直链） | ✅ **对象级下载已验**（20/20 + manifest）；HF 链路会间歇不可达，见 F 节 |
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

## F. 实测：已接入并跑通（2026-10-31 起，2026-11-01 复测）

- **脚本**：`scripts/fetch_textures.py`（ambientCG 子集：带 UA、重试、只留 Color、磁盘 <5 GB 停）、
  `scripts/prep_textures.py`（转 256²/128² 灰度 PNG）。
  ⚠️ 同目录的 `fetch_textures.sh`（curl 版）**已弃用**：Git Bash 下 curl 对 Windows 路径/CRLF 组合
  会静默产出 0 字节（实测 0/8 全失败），改 Python 后 8/8、14/14 成功。
- **吞吐（实测）**：单线程约 12 个/分钟；**8 线程并发 149 个/约 2.5 分钟**（`JOBS` 参数）。
  单个材质只留 Color ≈ **1 MB**；**149 个 = 159 MB** 原始 + 8 MB PNG。**300 个 ≈ 320 MB** 原始 + 17 MB PNG（256² 灰度）。
- **已训**（128² 灰度、hid 64、lr 0.01、**固定留出前 2 张**、300 张里取前 N 张训练）：

  | 训练图数 | 3×3 @60k | 7×7 @60k | 3×3 @100k | 7×7 @100k |
  |---|---|---|---|---|
  | 17 | 26.91 | 26.62 | — | — |
  | 147 | 28.25 | 28.33 | — | 30.32 |
  | 298 | 28.65 | 29.20 | 28.73 | **30.59** |

  ⇒ 数据量还在涨但已变平（3×3 17→298 张 +1.74 dB，147→298 只 +0.40）；**7×7 在 ~147 张起反超 3×3**，
  298 张时领先 0.55（60k）/1.86（100k）。7×7 在小预算配置（64² / hid 32 / lr 0.03 / 30k）下会崩到 3.6–10.5 dB，
  3×3 同预算是 27.3/27.9 —— 输在优化预算，不是表达力。
- ⚠️ **旧读数不可比（2026-11-01 更正）**：之前「149 张 29.04 / 19 张 27.74 ⇒ +1.3 dB」用的是**最后 2 张**做留出，
  换数据量时评测集也跟着变。现在的口径是固定留出前 2 张 + `--train-imgs`，跨规模只认上表。
- **全量账**：ambientCG 2,037 材质 × 1 MB(Color) ≈ 2 GB（不是 16 GB —— 之前按 zip 体积估高了）。
- `parts-3d tex-prior --data <png目录>` 与 `tex-gen --data <png目录>` 都已接真实材质。
  `tex-gen --data` 目前是**负结果**：20k 步能收敛，但采样塌成平图（std 0.001–0.002，均值比真实低 0.15–0.20），
  见 `GENERAL-PLAN.md` §6.21。

## G. Objaverse 对象级下载（2026-11-01）

- 脚本：`scripts/fetch_objaverse.py [OUT] [COUNT] [SHARD] [LICENSES]`。
  直接拉 `metadata/<shard>.json.gz`（5000 条，约 3.6 MB）+ `glbs/<shard>/<uid>.glb`，
  按 metadata 里的 `license` 过滤（默认 `cc0,by,by-sa`），GLB 魔数校验，写 `manifest.json`。
- 实测：shard `000-000` **20/20 下载成功**（总计 4 MB），manifest 里 uid/name/license/体积齐。
- 两个坑：
  1. 按体积**升序**取样本会全拿到 ~3 KB 的空壳 GLB（魔数合法、几何近乎空）⇒ 已改成
     **跳过 <200 KB、在 15 MB 上限内从大到小取**；
  2. 重取大物体时 **HF 整体不可达**：DNS 返回 `185.45.6.103`，`huggingface.co` root 与 resolve 直链
     都 15–25 s 超时（curl 000）。这是网络侧，不是脚本；换时段再跑，或 `curl --resolve` 钉可用 IP。
