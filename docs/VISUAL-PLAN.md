# 视觉理解扩展计划：光影 / 风格 / 视频提取 / 画质 / UI

> 2026-11-01。本文是**计划与调研**，不是结果。所有「已验」只标我们自己在仓里跑过的读数；
> 论文数字一律标来源，**未复现**的写成「论文声称」。
>
> 起因（用户原话归纳成 6 条能力）：
> 1. **光影**：能分辨一张图里的光（方向/强弱/色温/软硬），最好还能「还原出现实的光」；
> 2. **风格**：油画风 + 现实画质 + 赛璐璐 + 史诗风……**多层风格叠加**时模型能吸收/学习/提取多少；
> 3. **视频提取**：从视频里直接抠出物体/部件，**有账号水印或各种遮挡时也要能还原**；
> 4. **画质**：游戏/纪录片的画质梯度（含 4K 级）怎么被模型利用，类似 DLSS 的增强思路；
> 5. **UI 不是负样本**：软件界面也要吸收（UX/截图 → 可学习素材），除非明确只要照片；
> 6. **分辨**：先分清这是照片 / 游戏 CG / 截图 UI / 画作，再谈后面的处理。

## 0. 一句话结论（先给判断，再给方案）

现在这条线（分割头 + 体素融合 + 三值纹理去噪 + DDIM 采样）**完全没有光字段**：
`Camera` 只做几何投影，`tex` 只做灰度/颜色，`splat` 里 alpha 是占位。
所以「光影」不是调参能出来的，需要**新增一个本征分解分支**（albedo / shading / 光照参数），
并且它是**连续量**——按既定规矩：**三值网做分类与门，f32 头做连续量**，两者分开训、分开量。

风格与画质同理：**先把它们拆成可测的标量/向量，再谈叠加**，否则「油画+赛璐璐+史诗」只是一个词。

## 1. 现状盘点（2026-11-01 实测）

| 能力 | 现状 | 证据 |
|---|---|---|
| 部件分割 | 域内 0.8118（含 ~4.6× 颜色补贴，随机色 0.1800） | MODEL-CARD §4/§4f |
| 多物体实例 | S2 0.7965；S3 0.518（oracle 0.789） | GENERAL-PLAN §6.7–6.9 |
| 3D 重建 | N1 0.58–0.76、N2 0.582 且闭合 8/8 | §6.10–6.12 |
| 纹理 | 三值去噪 30.59 dB（1,297 张真实材质，固定留出） | §6.21–6.22 |
| 纹理**生成** | 卷积去噪器第一次出结构，但只有 ~8–16 px 低频斑块 | §6.23 |
| 光照 / 材质 | **没有**：无 albedo、无 shading、无光照参数 | — |
| 视频 | **没有**：目前只有多视角静态帧融合 | — |
| UI / 截图 | **没有单列**：OOD 负样本池是笼统的「非本类图」（含文档页/生成图/资产图/棚拍图，116 张） | MODEL-CARD §4d |


## 1.5 首批 10 个样本的实测读数（2026-11-01，`parts-3d look`）

用户给的 10 张（游戏/AI CG/动画/纪录片风/写实特写），**原始是手机整屏截图**（含 App 搜索栏、
评论、右侧点赞条）。原件落在 `D:/KF/trit-data/style-samples/`，另存 `*_frame.png` = 手工裁出的画面区
（不裁右侧头像/点赞条，它们不属于画面）。图**不进仓**（版权），只把数字写进来。

测量命令（同一输入两次跑逐位一致，见 `look` 模块单测）：

```bash
parts-3d look D:/KF/trit-data/style-frames --out features.csv
```

画面区读数（亮度/对比/饱和/色相熵/细纹/硬边/平坦/级数/雾感/发光/冷暖/主光°/光比/高光）：

| 样本 | 亮度 | 对比 | 饱和 | 色相熵 | 细纹 | 硬边 | 平坦 | 级数 | 雾感 | 发光 | 冷暖 | 主光° | 光比 | 高光 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 01 骑士+全息 | 0.43 | 0.72 | 0.31 | 0.34 | 0.269 | 0.027 | 0.39 | 61 | 0.24 | 0.011 | −0.09 | 8 | −0.12 | 0.010 |
| 02 动画冷舱 | 0.27 | 0.71 | 0.55 | 0.33 | 0.161 | 0.023 | 0.42 | 43 | 0.19 | 0.000 | −0.19 | 156 | −0.13 | 0.006 |
| 03 复古未来列车 | 0.56 | 0.75 | 0.36 | 0.72 | 0.320 | 0.067 | 0.21 | 58 | 0.35 | 0.077 | −0.01 | 102 | +0.08 | 0.038 |
| 04 FPS 实机 | 0.31 | 0.78 | 0.22 | 0.71 | 0.262 | 0.061 | 0.23 | 60 | 0.29 | 0.002 | −0.03 | 95 | −0.04 | 0.022 |
| 05 写实森林 | 0.21 | 0.66 | 0.26 | **0.17** | 0.176 | 0.025 | 0.36 | 50 | 0.19 | 0.000 | 0.00 | 55 | −0.03 | 0.006 |
| 06 Blender 短片 | 0.29 | **0.87** | 0.26 | 0.73 | 0.126 | 0.027 | 0.46 | 55 | 0.17 | 0.002 | +0.02 | 169 | +0.02 | 0.050 |
| 07 生物特写 | 0.25 | 0.50 | **0.63** | 0.27 | 0.126 | 0.009 | 0.68 | 47 | 0.13 | 0.006 | +0.18 | 138 | **+0.30** | 0.000 |
| 08 科幻冷舱 | 0.18 | 0.50 | 0.24 | 0.46 | 0.175 | 0.034 | 0.44 | 43 | 0.28 | 0.004 | 0.00 | 86 | +0.07 | 0.001 |
| 09 同步率屏 | 0.42 | 0.55 | 0.20 | 0.51 | 0.160 | 0.018 | 0.55 | 49 | 0.16 | 0.000 | −0.02 | 46 | +0.10 | 0.004 |
| 10 史诗沙漠 | 0.46 | 0.45 | 0.52 | 0.22 | 0.098 | 0.003 | 0.41 | 38 | 0.15 | 0.016 | **+0.30** | 89 | +0.25 | 0.004 |

**三条能直接落进计划的读数**：

1. **UI 污染是实测量级级的**：04 整屏 vs 画面区——亮度 0.56→0.31、**高光 0.431→0.022**（白底页面造出
   43% 假高光）、细纹 0.122→0.262。⇒ 任何风格/光影统计**必须先抠 UI 区**，这正是 S1 的理由，
   不是「顺手做一下」。
2. **光比（亮部−暗部冷暖差）能分开片子的打光**：07 生物特写 **+0.30**、10 沙漠 **+0.25**（暖主光/冷补光），
   02 动画冷舱 **−0.13**（冷光主导）。这是「能分辨一张图的光影」的第一个可复跑读数。
3. **「硬边=线稿」这个启发式不成立**：动画帧（02/09）的强梯度占比 0.023/0.018，和写实帧（05）0.025
   基本一样——截图缩放与视频压缩把线稿磨平了。⇒ 赛璐璐/写实必须靠**训练出来的判别头**（S1/S6），
   不能拿 `strong` 硬套；`look::guess` 在表里只作参考，不作结论。

**主光方向（`主光°`）现在只作代理量**：02 整屏报 12°、画面区报 156°，同一个场景因为 UI 像素参与
就翻了 144°。⇒ 方向必须建立在 shading 分解上（S2/S3），全局梯度朝向只能当弱特征。

## 2. 技术地图（按能力分块）

### 2.1 光影：本征分解 + 单图重打光

- **Intrinsic Image Diffusion**（CVPR 2024，[arXiv:2312.12274](https://arxiv.org/abs/2312.12274)）：
  单视图生成 albedo / roughness / metallic，**采样多个解释**以应对歧义；论文声称比 SOTA +1.5 dB PSNR、
  FID 好 45%，自述局限是「光照-材质歧义」与「缺真实数据」。
- **RGB↔X**（[项目页](https://github.com/tkuri/Awesome-InverseRendering) 里有索引）：把反照率/法线/光照拆成
  可往返的通道，做「分解 + 合成」双向监督。
- **LuxDiT**（NVIDIA，[PDF](https://research.nvidia.com/labs/toronto-ai/LuxDiT/assets/LuxDiT_paper.pdf)）：
  用视频扩散 Transformer 做**光照估计**（从动态内容里学光），对应我们要的「视频里也能算光」。
- **DiffusionLight / IllumiDiff / WildRelight**（[awesome 列表](https://github.com/tandaily/Awesome-Relighting)）：
  环境光估计与真实场景重打光的 benchmark 线；WildRelight 是 2026 的真实数据基准（**未复现**）。
- **IC-Light / Scaling in-the-wild Illumination**（[OpenReview](https://openreview.net/forum?id=u1cQYxRI1H)）：
  用数据规模换重打光质量，做法是「前景重打光 + 背景融合」，工程上比全套逆渲染现实得多。

**对我们的落点**：我们不需要先做「物理正确的 BRDF」，而是三件事：
① **光照方向 + 软硬 + 色温**（3+2 个 f32 标量）；② **shading 图**（单通道，可三值量化成「明/暗/中间」）；
③ **albedo 图**（去光后的颜色，用于纹理与重打光）。这三件都能用**公开模型的伪标签**先训小头，
不需要我们自己从零训一个逆渲染大模型。

### 2.2 风格：叠加、解耦、可测量

- **Style Transfer: A Decade Survey**（[arXiv:2506.19278](https://arxiv.org/html/2506.19278v1)）：
  从 Gram 统计到扩散的方法谱系，适合定「基线与指标」。
- **StyleStudio**（CVPR 2025，[poster](https://cvpr.thecvf.com/virtual/2025/poster/34070)）：
  文本驱动 + 参考图风格，强调**选择性控制**（只迁移风格、不破坏内容）。
- 扩散时代的通用手段：**LoRA / IP-Adapter / StyleDrop 类适配器**——多风格叠加的工程做法是
  「多 LoRA 权重插值」或「多参考注意力的加权融合」，而不是训一个「万能风格模型」。
- **赛璐璐（cel shading）**是可**解析**的：色阶量化 + 边缘线；**油画**是可**统计**的：笔触尺度、
  局部对比、色相抖动。**史诗风**主要是构图/对比/色温。⇒ 都能变成标量，不必当成玄学。
- 我们的两条约束：三值网不擅长连续风格强度 ⇒ **风格只用 f32 调制头**；风格词到参数的映射
  用小回归网（输入：参考图嵌入 + 统计量；输出：4–6 个风格因子）。

### 2.3 视频提取与去遮挡

- **VOS**：SAM2 一类「可提示分割 + 记忆」是当前主线（[awesome-VOS](https://github.com/gaomingqi/Awesome-Video-Object-Segmentation)）。
- **视频抠像**：SAM2Matting（跟踪与抠像解耦，**论文页**见 HF Daily Papers 2606.27339）。
- **去遮挡重建**：*Reconstructing Occluded Objects with Denoising Inpainters*（[arXiv:2503.10256](https://arxiv.org/abs/2503.10256)）：
  在 3DGS 场景里把被遮挡物体**提取出来 + 扩散修补**，思路与我们 `fuse` + 补洞同源。
- **视频物体移除/插入**：Attentive Eraser（AAAI 2025，[论文](https://ojs.aaai.org/index.php/AAAI/article/view/34285/36440)）、
  EffectErase（CVPR 2026，[PDF](https://openaccess.thecvf.com/content/CVPR2026/papers/Fu_EffectErase_Joint_Video_Object_Removal_and_Insertion_for_High-Quality_Effect_CVPR_2026_paper.pdf)）——
  后者明确处理**遮挡/阴影/光照/反射**这些「物体带出来的效果」，正好对应你说的「连光影一起提取」。

**对我们的落点**：我们已有 `fuse`（多视角体素投票 + 视觉外壳 + 补洞），把它从「多视角」扩成
「多帧 + 遮挡掩码」就是 MVS 版的视频提取；遮挡区用现有 `fill_solid` 先给几何、再谈纹理修补。

### 2.4 画质（类 DLSS）

- **Neural Super-Resolution for Real-time Rendering**（[arXiv:2308.06699](https://arxiv.org/html/2308.06699v2)）：
  利用引擎辅助缓冲（深度/法线/运动矢量）做实时 SR——**这类方法必须有引擎侧数据**。
- **PlaySR**（[OpenReview](https://openreview.net/pdf/723e89823dd6ff8c18d02a7228b5caec004c0d75.pdf)）：
  游戏渲染管线里的神经网络 SR。
- **FiDeSR**（CVPR 2026，[PDF](https://openaccess.thecvf.com/content/CVPR2026/papers/Kim_FiDeSR_High-Fidelity_and_Detail-Preserving_One-Step_Diffusion_Super-Resolution_CVPR_2026_paper.pdf)）：
  单步扩散 SR，强调「保真度 + 感知质量」的折中（**未复现**）。

**诚实的差别**：DLSS 之所以强，是因为它拿得到**引擎真值缓冲**；我们只有像素。
所以我们的「画质学习」只能是**后处理 SR/复原**（退化-恢复配对），不能宣称 DLSS 等价。
把它写进指标：同分辨率下 PSNR/SSIM 保底、LPIPS 感知提升，**不许只报感觉**。

### 2.5 UI / 截图：从「负样本」改成「正类」

- GUI 元素检测已有成熟线：YOLO 系（[arXiv:2408.03507](https://arxiv.org/html/2408.03507v1)）、
  Roboflow 的 [UI 数据集索引](https://universe.roboflow.com/search?q=class%3Aui)；
  XUI（[ACM](https://dl.acm.org/doi/full/10.1145/3564702)）做「截图 → 自然语言描述」。
- 我们要的其实很轻：**三类判别头**（照片 / CG / UI）+ UI 区域掩码。
  现状是 OOD 门只有「本类 / 非本类」两分，负样本池里混着文档页、生成图、资产图（MODEL-CARD §4d），
  **UI 并没有被当成一类去学** ⇒ 要拆成三类：UI 单独一类，**参与训练但走自己的头**（不是当负样本）。
- 吸收方式：UI 的「布局/层级/控件」是另一种结构化数据，适合同一套「平面头 + 部件头」思路
  （控件 = 部件，层级 = 父子关系），不是塞进照片类里当噪声。

## 3. 落到本仓的分工（三值 vs f32）

| 模块 | 形态 | 理由 |
|---|---|---|
| 三类判别（照片/CG/UI） | 三值 CNN/MLP 分类头 | 离散决策，三值足够 |
| 光照方向/强度/色温 | f32 小回归头（3+2 维） | 连续量（既定规矩） |
| shading 图 | 三值量化（明/中/暗）+ f32 残差 | 离散结构 + 连续细节 |
| albedo 图 | f32（颜色是连续量） | 与现有纹理 head 一致 |
| 风格因子（4–6 维） | f32 调制向量 | 连续、可插值 |
| 视频时序融合 | 现有 `fuse` 扩展（体素投票） | 已跑通，改输入即可 |
| 遮挡修补 | 先几何 `fill_solid`，后 2D 扩散 | 分两步，先保证闭合 |
| 画质 SR | f32 卷积/扩散（后处理） | 不是三值能扛的连续重建 |

## 4. 分步计划（每步有判据，做不动就停）

**S1 · UI 正类化（0.5–1 天）**
- 做：拆 OOD 负样本池 → 新增 `ui` 类；三类头（照片/CG/UI）训练与门。
- 判据：三类留出混淆矩阵；UI 召回 ≥0.95 且照片误判 UI ≤2%。
- 风险：UI 数据集多为研究许可，只能自采截图 + 少量公开集。

**S2 · 光照标量头（2–3 天）**
- 做：用公开本征分解模型离线跑伪标签（albedo/shading），
  训 f32 头回归「主光方向（球坐标）+ 强度 + 色温 + 阴影软硬」。
- 判据：光照方向角误差中位数 < 15°；shading 图 PSNR 相对基线 +2 dB。
- 风险：伪标签自带偏差 ⇒ 先在同分布（室内/材质球）上验，再谈真实照片。

**S3 · shading 三值量化（1–2 天）**
- 做：把 shading 图按分位数三值化，训三值网预测「明/中/暗」图，残差交 f32。
- 判据：三值图与连续图的互信息 ≥0.8；下游重打光 PSNR 不掉超过 0.5 dB。

**S4 · 视频逐帧提取 + 时序融合（3–5 天）**
- 做：逐帧部件头 → 光流/长期记忆对齐 → 体素投票（复用 `fuse`）；输出逐帧 id 与置信。
- 判据：同一序列相邻帧 IoU 抖动 <5%；遮挡后重出现时 id 恢复率 ≥80%。

**S5 · 遮挡掩码 + 修补（2–4 天）**
- 做：掩码 = 分割外/低置信区 + 时序不可见区；几何先 `fill_solid`，纹理用扩散修补（后续接）。
- 判据：掩码区几何闭合（边界边 = 0）且体积误差 <15%；纹理修补区 PSNR ≥ 非掩码区 −3 dB。

**S6 · 风格因子与叠加（3–5 天）**
- 做：定义 4–6 个可测因子（笔触尺度、色阶数、边缘强度、局部对比、色温、饱和度）；
  用参考图回归它们；做「油画 × 赛璐璐 × 史诗」的因子插值实验。
- 判据：因子可复现（同图两次 <2% 抖动）；插值生成的风格距离（Gram/CLIP）单调变化。
- 风险：**游戏美术风格有版权**，只能本地自采少量、不进公开数据集。

**S7 · 画质退化-恢复（3–5 天）**
- 做：配对退化（降采样/压缩/噪声/去色阶），先 2× SR；只在视频后处理路径用。
- 判据：PSNR/SSIM 不低于基线，LPIPS 下降 ≥10%；**明确不宣称 DLSS 等价**。

**S8 · 端到端演示（收口，2–3 天）**
- 做：一段视频 → 逐帧部件 + 遮挡修补 → 3D 融合 → 光照/风格因子 → 合成图。
- 判据：能出一条可复跑的命令链 + 一份读数表；不达标项如实列出。

## 5. 数据与许可

| 用途 | 来源 | 状态 |
|---|---|---|
| 材质/纹理 | ambientCG（1,299 张已落盘）、Poly Haven | ✅ 已验，CC0 |
| 本征分解伪标签 | 公开模型权重（待选：Intrinsic Image Diffusion 许可需核） | ⚠️ 未核许可 |
| UI | 自采截图 + Roboflow/RICO 类公开集 | ⚠️ 多为研究用途 |
| 游戏画面 | **只能本地自采**，不入库、不训练进公开模型 | 🔴 版权 |
| 视频 | 自采/授权素材 | ⚠️ 逐条确认 |

## 6. 指标与门（不写「感觉更好」）

- **光影**：光照方向角误差（中位数/95 分位）、shading PSNR、重打光后 LPIPS。
- **风格**：Gram 距离、CLIP 风格相似度、因子复现抖动；人评只作补充。
- **视频/去遮挡**：时序 IoU 抖动、遮挡恢复率、掩码区 PSNR/LPIPS、几何闭合（边界边 = 0）。
- **画质**：PSNR/SSIM 保底 + LPIPS 感知提升，**同输入同种子**对比。
- **UI**：三类混淆矩阵 + 每类召回。
- 通用：所有实验固定随机种子、固定留出、两种子上报（沿用 §6.22 的教训）。

## 7. 风险与诚实边界

1. **「还原真实的光」是逆渲染问题**，单图有本质歧义（材质 × 光照不可分）；只能给**最可能的解释**，
   不能承诺物理唯一解。论文自己也把这条列为主要局限。
2. **严重遮挡**（账号水印、整块覆盖）只能「合理猜测」；几何能补闭合，纹理只能生成。
3. **风格叠加**不是线性可加的：多 LoRA 会在高权重打架；我们先用**可测因子插值**做，
   不要一上来训「万能风格模型」。
4. **DLSS 类增强需要引擎缓冲**，我们只有像素 ⇒ 只能做后处理，指标上不许混淆。
5. **UI 不是负样本**：这是当前管线的设计错误，S1 就要改，否则会一直把 UX 素材当噪声。
6. 三值网络在**连续量**上不要硬扛：光照/风格/画质一律 f32 头，三值只做离散结构与门。

## 8. 引用

- [Intrinsic Image Diffusion (CVPR 2024) — arXiv:2312.12274](https://arxiv.org/abs/2312.12274)
- [LuxDiT: Lighting Estimation with Video Diffusion Transformer (NVIDIA)](https://research.nvidia.com/labs/toronto-ai/LuxDiT/assets/LuxDiT_paper.pdf)
- [Awesome-InverseRendering（本征分解/逆渲染索引）](https://github.com/tkuri/Awesome-InverseRendering)
- [Awesome-Relighting（重打光索引，含 IllumiDiff/DiffusionLight）](https://github.com/tandaily/Awesome-Relighting)
- [Scaling In-the-Wild Training for Diffusion-based Illumination（OpenReview）](https://openreview.net/forum?id=u1cQYxRI1H)
- [Style Transfer: A Decade Survey (2025)](https://arxiv.org/html/2506.19278v1)
- [StyleStudio (CVPR 2025)](https://cvpr.thecvf.com/virtual/2025/poster/34070)
- [Awesome-Style-Transfer-with-Diffusion-Models](https://github.com/Westlake-AGI-Lab/Awesome-Style-Transfer-with-Diffusion-Models)
- [Reconstructing Occluded Objects with Denoising Inpainters (2025)](https://arxiv.org/abs/2503.10256)
- [Attentive Eraser (AAAI 2025)](https://ojs.aaai.org/index.php/AAAI/article/view/34285/36440)
- [EffectErase (CVPR 2026, 视频物体移除+光影效果)](https://openaccess.thecvf.com/content/CVPR2026/papers/Fu_EffectErase_Joint_Video_Object_Removal_and_Insertion_for_High-Quality_Effect_CVPR_2026_paper.pdf)
- [Awesome-Video-Object-Segmentation](https://github.com/gaomingqi/Awesome-Video-Object-Segmentation)
- [Neural Super-Resolution for Real-time Rendering — arXiv:2308.06699](https://arxiv.org/html/2308.06699v2)
- [PlaySR: Real-time Super Resolution in Gaming](https://openreview.net/pdf/723e89823dd6ff8c18d02a7228b5caec004c0d75.pdf)
- [FiDeSR (CVPR 2026, one-step diffusion SR)](https://openaccess.thecvf.com/content/CVPR2026/papers/Kim_FiDeSR_High-Fidelity_and_Detail-Preserving_One-Step_Diffusion_Super-Resolution_CVPR_2026_paper.pdf)
- [GUI Element Detection with YOLO — arXiv:2408.03507](https://arxiv.org/html/2408.03507v1)
- [XUI: Describing UI Screenshots in Natural Language (ACM)](https://dl.acm.org/doi/full/10.1145/3564702)
- [Roboflow UI 数据集索引](https://universe.roboflow.com/search?q=class%3Aui)
