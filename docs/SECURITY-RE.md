# 网安 / 逆向技术调研：PYAS、KSword 与静态二进制分析

> 目的：给 `trit-re`（本仓的二进制逆向台账）划清"该做什么、不该做什么"。
> 调研快照：2026-10-30。外部项目按当日 `default` 分支 HEAD 读取，星数/体积会变。
>
> **一句话结论**：PYAS 与 KSword 是"产品级安全工具链"——一个把静态/云端/内核防护整合成
> 终端产品，一个是 Windows R3/R0 交叉视图的 ARK 工作台；`trit-re` 不是它们的替代品，
> 而是一个**可复算的静态证据台账**：只回答能从文件本身证明的问题，证不了就写"未知"。

## 0. 证据等级

本文每条结论标注来源强度：

| 等级 | 含义 |
|---|---|
| **A** | 读到的源码/仓库文件原文（给出文件路径），或本机实测读数 |
| **B** | 项目 README / 元数据声明，**未逐行核验实现深度** |
| **C** | 论文/综述的结论或一般性推断，未在本机复算 |

---

## 1. 两个项目做了什么

### 1.1 PYAS（87owo/PYAS）

- 快照：HEAD `ac04f4b`，590 stars / 51 forks，仓库约 **1.3 GiB**，主语言标注 YARA，
  许可证非 OSI（`NOASSERTION`）。**B**
- 定位（README）[**B**]：Windows 终端安全平台 = YARA + PE 机器学习（ONNX）+ Authenticode +
  可选云查 + 用户态实时监控 + 原生 minifilter 驱动。
- 静态分析面 [**A**，`PYAS_Features.py` / `PYAS_PE.py`]：
  - `FileEntropy`、按窗口累积的**字节熵直方图**（16 个熵档 × 16 个字节档）、
    `StringCount` / `StringMeanLength` / `StringEntropy`；
  - 叠加段特征 `HasOverlay` / `OverlaySize` / `OverlayRatio` / `OverlayEntropy`；
  - 资源区间、DLL 名的 **512 维哈希**（hashing trick，`dll_hash_dim = 512`）、
    模型特征顺序从 `features.json` 读取；
  - 推理走 ONNX Runtime，输入是 `numpy.float32` 向量（`PYAS_PE.py`）。
- 规则面 [**A**]：`Engine/Heuristic/Rules_General_A1.yar` 单文件 **8033 行**——
  288 条公开规则 + 2 条 `private`，含家族级规则（例如 `Backdoor_WinPE_CobaltStrike` 的字符串锚点 +
  字节码模式）。规则的 `meta` 里带 `scope` / `evidence` / `attribution` ——
  **作者把"证据"与"归因"分开写**，这一点值得借鉴。
- 内核面 [**A**，`Plugins/Filter/`]：minifilter 驱动按 `ProtectBoot/File/Memory/Process/Registry`
  分文件，配 `.inf` 与规则 JSON。
- 需要注意 [**B**]：训练脚本与模型都在仓里，但**云端服务端不在公开仓**；ML 特征语义取决于
  未公开的 `features.json`，不能仅凭 README 断言它等价于 EMBER 特征集。

### 1.2 KSword（WangWei-CM/KSword）

- 快照：HEAD `41c21ac`，588 stars / 117 forks，约 **185 MiB**，C++，许可证为
  "KSword Community Source License v1.6"（**非 OSI 认证**）。**B**
- 定位（README）[**B**]：Windows 10/11 x64 的 ARK（反 rootkit）与系统分析套件；
  核心方法是**用户态与 Ring 0 视图交叉比对**，把"差异"当作调查证据，同时明确要求
  先排除采样时刻与合法系统行为。
- 能力面（README 声明，未逐项核验实现）[**B**]：
  - 进程/线程/模块/句柄的 R3/R0 交叉视图；SSDT/SSSDT、Inline/IAT/EAT 钩子；
    内核回调清单与监控；ETW、syscall 捕获；
  - 内存工作台（区域浏览、搜索、PTE/VA 转换）、PE/ELF/Mach-O 扫描、NTFS 取证；
  - HVM 页：嵌套 VMX / 实验性 SVM、EPT 视图与钩子、SLAT/IOMMU 证据；
  - 自带 MCP 接入说明，可被 AI 客户端调用（同一作者的另一仓）。
- 与 PYAS 的差别：PYAS 面向"防护产品"，KSword 面向"调查/调试工作台"；
  两者都**依赖内核驱动**，而 `trit-re` 明确停在用户态静态文件。

---

## 2. 能力矩阵：它们与 `trit-re` 的关系

| 证据面 | PYAS | KSword | `trit-re`（当前） |
|---|---|---|---|
| PE/ELF 结构解析 | pefile（Python） | 自带扫描器 | 手写、零第三方依赖 |
| 熵 / 叠加段 / 壳线索 | 有（特征向量） | 有（文件页） | 有（节熵、叠加段、壳节名、入口） |
| 导入能力归类 | 未见（未核验） | 未见（未核验） | **有**（9 个能力桶 + 三值事实） |
| 导入函数交叉引用 | — | IAT/EAT 钩子检查（运行期） | **有**（静态，x86/x86-64/ARM64） |
| 函数级归属 / 同函数共现 | — | 反汇编页 | **有**（PE `.pdata` / ELF `STT_FUNC`） |
| YARA 匹配 | **核心**（288 条规则） | — | 无 |
| 签名验证 | WinVerifyTrust | 签名操作 | 只识别 signedData 结构，**不验证信任链** |
| 进程/内核/网络/内存 | 有（+minifilter） | **核心**（R3/R0 交叉视图） | 无（由 `trit-procscan` 负责进程面） |
| 虚拟机 / 反 rootkit | — | HVM、回调、钩子 | 无 |
| 输出可复算台账 | 评分/告警 | UI 证据 | **JSONL + sha256 主键** |

结论：三者不是替代关系。**PYAS 的熵/叠加段/DLL 哈希特征和 `trit-re` 的事实层高度重叠**，
但 `trit-re` 不做模型评分、不做 YARA；**KSword 的 IAT/EAT 检查发生在运行期**，
而 `trit-re` 回答的是"文件里有没有这个调用"。`trit-re` 的差异化只有两点：
**三值语义**（未知不被当成"没有"）和**可复算台账**。

---

## 3. 技术脉络：哪些论文支撑我们的设计，哪些是我们要刻意不碰的

### 3.1 三值不是我们发明的

- Cousot & Cousot 1977 的抽象解释给出了"用格上的近似值做静态分析"的框架。**C**
- Sagiv / Reps / Wilhelm 的 *Parametric Shape Analysis via 3-Valued Logic*
  （POPL 1999；TOPLAS 2002）把"不确定"显式建模成三值逻辑的中间值，并用它证明分析的
  可靠性。**C**（[ACM](https://dl.acm.org/doi/10.1145/292540.292552)）

**批判**：TVLA 的三值是**抽象域里的"不知道"**（may/must 合并），`trit-re` 的三值是
**证据链上的"没拿到"**（解析失败、模式未覆盖）。两者都叫 Unknown，但语义不同：
我们的 `False` 不承诺"程序性质为假"，只承诺"在当前证据范围内没找到"。文档与 API 里
必须一直这么写，否则会把"没找到"误读成"不存在"。

### 3.2 静态 PE 机器学习的公开基线

- EMBER（Anderson & Roth, 2018）给出了静态 PE 的特征分组与 1.1M 样本基准
  （文件信息、头、导入/导出、节、字节直方图、字节熵直方图、字符串、数据目录）。
  **C**（[arXiv:1804.04637](https://arxiv.org/abs/1804.04637)）

PYAS 的 `FileEntropy` / 熵直方图 / `StringEntropy` / 叠加段特征与 EMBER 的分组**同源**，
但它多了叠加段比例与 DLL 哈希，少了明确的导入/导出符号特征。**B**（特征名 [A]，
"同源"是结构对照 [B]）。对 `trit-re` 的启示：如果要走模型路线，特征定义必须像 EMBER
一样公开、可复算，否则读数不可比。

### 3.3 二进制相似性不是导入交叉引用

- Haq & Caballero 的 *A Survey of Binary Code Similarity* 系统化了 70 种方法
  （应用、特征、算法、评测）。**C**（[ACM](https://dl.acm.org/doi/10.1145/3446371) /
  [arXiv:1909.11424](https://arxiv.org/abs/1909.11424)）

`trit-re` 的 xref 只到**导入级**：谁调用了 `CreateRemoteThread`，不回答"两个函数是否相似"。
若要做函数相似，输入需要 CFG / 指令特征 / 嵌入，这会把 crate 的零依赖与"可复算"两条
同时打破。**明确列为不做**。

### 3.4 全量二进制分析平台的能力边界

- *SoK: (State of) The Art of War: Offensive Techniques in Binary Analysis*
  （S&P 2016）描述了 angr 体系下的 CFG 恢复、VEX IR、符号执行、动态追踪等。
  **C**（[IEEE](https://ieeexplore.ieee.org/document/7546500/)）

它也是 `trit-re` 的"天花板参照"：真逆向需要 IR 与 CFG。我们的定位是**在 IR 之前**：
只做字节级模式 + 结构解析，把"不明"留给 `Unknown`。

### 3.5 加壳：熵只是线索

- Yason, *The Art of Unpacking*（Black Hat USA 2007）与 Roundy & Miller,
  *Binary-code obfuscations in prevalent packer tools*（ACM CSUR 2013）都说明：
  加壳/混淆的手段（多态、虚拟化、反调试）远超"高熵"一个指标。**C**
  （[ACM](https://dl.acm.org/doi/10.1145/2522968.2522972)）

这直接支撑 `trit-re` 的四值设计：**"结构像壳"与"有签名"同时成立时给 `B`**，
而不是用阈值硬压成"恶意"或"良性"。

### 3.6 LLM 辅助逆向：另一个量级

- ReSym（CCS 2024）用 LLM + 程序分析恢复变量名与类型。**C**
  （[ACM](https://dl.acm.org/doi/10.1145/3658644.3670340)）

这类工作依赖反编译器产出的伪代码；`trit-re` 没有反编译器，**不做名称/类型恢复**。
若未来接入，应当作为独立层消费台账，而不是塞进三值判定。

---

## 4. 对 `trit-re` 的批判性评估

### 4.1 站得住的部分

- **三值一致性**：事实层、判定层、台账层是同一个 `Ternary`；证据缺失会一路传播到 JSONL。
- **可复算**：`sha256` 主键 + 固定 `schema` + 逐条证据字符串；同一文件重跑逐字节稳定。
- **真实正例**：mingw `-O1` 编译的注入样本 `fn_injection = True`（函数 `0x14d0+0xe5`
  同时含申请/写/执行三个桶），说明"同函数共现"不是纸面功能。
- **误报控制**：调用点必须精确命中 IAT/GOT 槽位；导入跳板与 PLT 桩会被展开到真正调用方，
  实测把大二进制（26 MB）的归属调用点从 14 提到 2782。

### 4.2 站不住 / 未证的部分

| 问题 | 现状 | 后果 |
|---|---|---|
| 模式匹配不是反汇编 | 只识别固定编码（`FF /2`、`adrp+ldr+blr` 等） | 寄存器经 `mov` 搬运、栈传递的间接调用会漏；数据里的疑似指令可能误报 |
| 无真实 ARM64 样本 | 只有合成夹具 | ARM64 路径的**误报率完全未知** |
| PE32 / 无 `.pdata` | 函数边界退化为启发式 | 同函数共现一律 `Unknown` |
| 无 YARA / 无签名验证 | 只识别 signedData 结构 | 不能替代 PYAS 的规则层 |
| 无动态/内核面 | 只看文件 | KSword 的 R3/R0 差异、钩子、回调都看不到 |

### 4.3 与两个项目的正确关系

- 对 PYAS：`trit-re` 输出的事实在"模型之前"——可以作为特征源或解释层，
  但**不提供评分**，也不假装能替代 288 条 YARA 规则。
- 对 KSword：`trit-re` 提供的是"文件里有什么调用"的静态底稿；运行期 IAT/EAT 是否被改，
  必须由 KSword 这类工具回答。两者拼起来才是完整证据链。

---

## 5. 路线图（每条都带判据）

| 编号 | 事项 | 判据（可复算） |
|---|---|---|
| N1 | **真实语料校准**：PE x64 / PE ARM64 / ELF x64 / ELF ARM64 各一批 | 记录每个样本的 `sha256`；人工复核 xref 误报/漏报，给出比例；ARM64 至少一批非合成样本 |
| N2 | **最小 CFG**：基本块 + 直接跳转边，替换启发式边界 | `unattributed_sites` 在 notepad/trit-re.exe 上下降；`fn_*` 判定不回归；PE32 共现从 `Unknown` 变为可判 |
| N3 | **接 `trit-procscan`**：现场模块路径 → 台账 | 一次采集产出 ≥ N 条 `xref`/`function` 记录，耗时与文件大小线性 |
| N4 | **YARA 子集的三值匹配** | 与官方 YARA 在同一批样本上的命中集合一致；"未命中"只在完整扫描后给出 |
| — | **不做**：完整反汇编、符号执行、内核驱动、动态行为 | — |

---

## 6. 复算命令

```bash
# 单元 + 集成测试（含合成 PE/ELF、x86-64 与 ARM64）
cargo test -p trit-re

# 人读摘要 / JSONL 台账 / 交叉引用明细 / 函数明细
cargo run -q -p trit-re -- triage <FILE>
cargo run -q -p trit-re -- triage <FILE> --jsonl
cargo run -q -p trit-re -- triage <FILE> --xref --functions

# 真实正例：mingw gcc -O1 编译后，期望 fn_injection = True
gcc -O1 -o inject.exe crates/trit-re/tests/fixtures/inject_sample.c
cargo run -q -p trit-re -- triage inject.exe --functions
```

## 7. 参考资料

- PYAS：<https://github.com/87owo/PYAS>（HEAD `ac04f4b`，2026-10-30）
- KSword：<https://github.com/WangWei-CM/KSword>（HEAD `41c21ac`，2026-10-30）
- EMBER：<https://arxiv.org/abs/1804.04637>
- Binary Code Similarity 综述：<https://dl.acm.org/doi/10.1145/3446371>
- 三值形状分析：<https://dl.acm.org/doi/10.1145/292540.292552>
- SoK / angr：<https://ieeexplore.ieee.org/document/7546500/>
- ReSym：<https://dl.acm.org/doi/10.1145/3658644.3670340>
- Packer 混淆：<https://dl.acm.org/doi/10.1145/2522968.2522972>
