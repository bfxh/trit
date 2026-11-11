# 隐私存储（后期任务）：威胁模型、技术路线与硬边界

> 2026-11-11 立项。本文是**计划与调研**，不是结果；后期实现时按 STATUS.md 的四个状态词重写进度。
> 题目原话（用户）：照片 / 视频 / 文本的隐私要“系统组件检测不到，但还能通过软件正常打开”，
> 担心“有些系统会扫盘、上传隐私文件”；希望做成底层组件（“编译器 / 转接器”这一层）。
>
> **定位**：保护**自己的数据**不被不可信的系统组件与云客户端扫描、索引、上传。
> 不针对执法取证，不用于规避合法调查；实现上也不做内核驱动（见 §2、§6）。

## 0. 一句话结论

**可达**：盘上数据对“内容扫描者”（杀软、系统索引器、云同步客户端、云端审核）表现为随机噪声——
无魔数、无扩展名、无缩略图、无明文文件名与目录结构；只有我们自己的挂载器 + 密钥能还原成
“系统播放器/编辑器能直接打开”的普通文件。
**不可达**：系统内核/EDR 在**运行时**读进程内存、屏幕采集（Recall 类）、SSD 磨损均衡留下的写入痕迹。
这三条是物理与信任边界问题，不是代码问题——任何宣称“全都能藏住”的方案都是假的。

## 1. 威胁模型（谁在看、看什么）

| 级别 | 扫描者 | 能力 | 典型泄露点 |
|---|---|---|---|
| T1 | 杀软 / 系统索引 / 云同步客户端 | 用户态遍历磁盘：扩展名、魔数、EXIF、缩略图、OCR/AI 分类 | 明文文件、缩略图缓存、`Recent` 列表、回收站 |
| T2 | 内核监控 / EDR / 屏幕采集（Recall 类） | 读进程内存、系统调用、截图 | 打开时的明文内存、屏幕上的画面、剪贴板 |
| T3 | 云端内容审核 | 同步后的服务端扫描（哈希匹配、OCR、AI） | 任何被同步上去的明文/密文侧信道 |
| T4 | 物理 / 固件 | 冷启动、DMA、SSD 磨损均衡与 TRIM | “隐藏卷”写入痕迹、内存残影 |

要保护的资产不只是一张图片：**内容是资产，元数据（EXIF/时间线/文件名/目录结构）、缩略图、
索引、回收站、备份、云端副本同样是资产**。只看“文件加密”一定会漏在缩略图和缓存上。

参考事实：Recall 是**系统级**屏幕采集（本地加密、由 OS 组件生成）——只要明文上屏就有副本；
这不是加密强度问题，是采集点在我们控制之外（见 [MS Recall 隐私说明](https://support.microsoft.com/en-us/windows/privacy/privacy-and-control-over-your-recall-experience)）。

## 2. 能达成 / 不能达成（硬边界，先写死）

**能达成（有明确验收口径）**
- 盘上静态数据对 T1/T3：密文分块 + 尺寸填充后与随机噪声不可区分；无明文元数据。
- 文件名、目录树、时间线：全部加密（目录树本身也是密文记录）。
- 只有“挂载器 + 口令/密钥”能打开；系统播放器/编辑器**通过挂载出来的普通文件接口**正常打开。
- 缩略图/预览**只在内存生成、不落盘**（这是最容易泄的地方，必须设计进去）。

**不能达成（写清楚，防止后面自欺）**
- T2：系统内核/EDR 能读我们的进程内存；打开时明文在内存里，存在即可读。
  （可选加固：`mlock`/`VirtualLock` + `zeroize` + 禁休眠换页；只能抬高成本，不能消除。）
- Recall 类屏幕采集：明面上屏 = 被拍。只能依赖系统开关/应用排除策略，属**部署问题**。
- T4：SSD 的磨损均衡/TRIM 会让“隐藏卷”的写入模式留痕；已有论文专门攻破该类可否认性
  （见 §4 的 VeraCrypt hidden OS 攻击）。可否认性在 SSD 上必须单独设计（全盘随机化 + 恒定写入，
  代价高），**默认不承诺**。
- 运行时隐蔽（进程/窗口/文件名被系统一眼看到）不在本任务目标内；本任务只保证**静态数据**。

## 3. 技术路线（分五层，全部 Rust）

### 层 1：容器格式（`trit-vault`，核心）
- 分块 AEAD：XChaCha20-Poly1305，块 64 KiB，随机 nonce + 块序号绑定（防重排/截断/拼接）。
- 密钥派生：Argon2id（每容器随机盐；支持多口令密钥槽，便于轮换）。
- 元数据：加密目录树 + 随机文件名映射；尺寸按 4 KiB 粒度填充（可选“恒定大小”模式）。
- 完整性：每块 AEAD + 顶层清单 Merkle 根；口径与 `trit-codec` 的清单/校验对拍。
- 破坏性变异测试：改一位、截断、重排、换容器头必须**结构化报错**，不许 panic。

### 层 2：可否认（可选，高风险，单独评审）
- 隐写容器：把密文嵌入照片/视频冗余域——容量/鲁棒性/可检测性是三角，
  AI 隐写分析（2025–2026 综述）已说明“检测不到”只是相对概念；**只建议用于小文本**。
- 隐藏卷（VeraCrypt 式）：在 SSD 上有已知痕迹攻击（§4），默认不采用；
  若要做，必须先给出抗 TRIM/磨损的写入模式设计与实测。

### 层 3：适配器（用户说的“转接器”，真正的落点）
> 注意：这一层**不是编译器**。编译器解决“代码怎么生成”，藏不住“数据在盘上长什么样”；
> 真正要写的是**存储适配器**（虚拟文件系统），让未修改的系统软件看到一个普通盘/目录。
- Windows：**WinFsp**（用户态文件系统，Rust crate `winfsp`），挂载为盘符或目录；不写内核驱动
  （避免驱动签名、蓝屏与系统升级兼容问题）。
- Linux：**FUSE**（`fuser` crate）。macOS：FUSE 或新版 **FSKit**。
- 系统兼容：正常文件接口（资源管理器/播放器/编辑器直接用）；云同步目录排除、索引器排除
  （避免“挂载点被同步客户端扫一遍”）。

### 层 4：运行时保护
- 密钥与明文缓冲区：`mlock`/`VirtualLock`、`zeroize`、拒绝休眠时的密钥落盘。
- 可选硬件信任根：TPM/VBS（Windows）、Apple SEP、Intel TDX/SGX——密钥不落盘，提升 T2/T4 成本。

### 层 5：内容形态
- 照片/视频：**整文件加密**（推荐），不做“像素级隐藏”。
- 文本：压缩 + 分块 + 加密。
- 缩略图/预览：内存生成、不落盘；禁止写入系统缩略图缓存。

## 4. 参考与论文（按可信度标注）

- **DEFY: A Deniable, Encrypted File System**（NDSS 2015）——可否认文件系统的设计与攻击面。
  [PDF](https://www.ndss-symposium.org/wp-content/uploads/2017/09/04_5_1.pdf)
- **Deniable Encryption**（Canetti–Dwork–Naor–Ostrovsky, CRYPTO'97）——可否认性的理论基础。
- **Defeating Plausible Deniability of VeraCrypt Hidden Operating Systems**（2026）——SSD/系统痕迹
  攻击的实证，直接支撑 §2 的 T4 硬边界。
  [ResearchGate](https://www.researchgate.net/publication/318155607_Defeating_Plausible_Deniability_of_VeraCrypt_Hidden_Operating_Systems)
- **Rethinking the adversary and operational characteristics of deniable storage**（2021）——运营特征
  （“随身携带加密盘本身就可疑”）必须进威胁模型。 [OAE](https://www.oaepublish.com/articles/jsss.2020.22)
- **图像隐写与检测综述**（ACM CSUR 2025；Springer 2026；Elsevier 2026）——AI 隐写分析现状，
  支撑“隐写只用于小文本”的判断。
  [ACM](https://dl.acm.org/doi/10.1145/3694965) · [Springer](https://link.springer.com/article/10.1007/s10791-026-10267-x) · [Elsevier](https://www.sciencedirect.com/science/article/abs/pii/S1574013726000018)
- **WinFsp 官方文档**——Windows 用户态文件系统；Rust 绑定 `winfsp`。
  [winfsp.dev](https://winfsp.dev/) · [docs.rs/winfsp](https://docs.rs/winfsp/latest/winfsp/filesystem/index.html)
- **Microsoft Recall 隐私说明**——系统级屏幕采集的采集点与开关。 [链接](https://support.microsoft.com/en-us/windows/privacy/privacy-and-control-over-your-recall-experience)

## 5. 里程碑（后期，带判据）

| 阶段 | 交付 | 判据（可复算） |
|---|---|---|
| P0 | 本文（威胁模型 + 判据） | 文档评审通过；红线写死 |
| P1 | `trit-vault` 容器格式 + CLI | `cargo test -p trit-vault`；改一位/截断/重排逐类结构化报错；`trit-fuzz` 5 万例无 panic |
| P2 | 只读挂载（Linux FUSE） | 挂载点外 `strings`/索引器扫不到明文；系统播放器能打开照片/视频 |
| P3 | Windows WinFsp 读写 + 崩溃一致性 | 进程被 kill 后容器可恢复或明确拒绝；无半写目录 |
| P4 | 系统兼容清单 | 脚本化检查：Defender/索引/云同步排除项；挂载点不被扫描 |
| P5 | 可否认/隐写（可选） | 容量/鲁棒性/检测率三项读数；单独威胁评审通过 |

红线：不做内核驱动；不做对抗执法取证；默认拒绝任何明文落盘（含缩略图/预览/回收站）。

## 6. 与主库的关系

- 新 crate 归 **ops 层**（存储与编码原语），计划名 `trit-vault`；依赖标准密码学 crate
  （`chacha20poly1305`、`argon2`），不引入外部模型。
- 与 `trit-codec` 的容器/清单口径对拍；门禁接入 module budget、test surface、mutation gate。
- 本任务**不排期**；先记录在 ROADMAP §2 的“该新开的”一栏，触发条件是用户明确启动。

## 7. 待决问题（启动前必须回答）

1. 平台优先级：Windows 本机优先，还是 Linux 先行？
2. 是否接受“打开时内存可见”（T2 不可防）这一边界？
3. 可否认性是必需还是可选（SSD 上成本/风险高，默认不做）？
4. 隐写只用于小文本是否可以？
5. 与系统云同步的关系：挂载点排除是否足够，还是要做“只读副本 + 幂等导出”？
