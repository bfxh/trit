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
- **未归属调用点的逐条裁定**（2026-11-03）：每个归不到函数区间的调用点按证据分类——
  疑**边界裁剪**（距最近边界 ≤16B：对齐填充/`.pdata` 只记函数体导致的尾部代码）、
  疑**边界漏检**（exec 内、距边界 >16B：某函数起点没被任何边界源识别）、
  **非执行节**（字节模式误报候选）。明细 `ruled_sites`（≤64 条）+ 三类计数进
  `FunctionReport`，方向保守不变（裁定不洗成"已归属"，共现的 `Unknown` 拖累逻辑不动）。
  这把「N 个未归属」从黑箱计数变成能直接看边界质量问题出在哪的证据清单——
  也是 N2（最小 CFG）成效的**现成度量**：边界漏检类应随 N2 显著下降。
- **最小 CFG 第一层：补隙合并**（2026-11-05）：直接控制流边收割（x86 `E8`/`E9`、
  ARM64 `BL`/`B`，字节模式级）+ 入口/目标定起点——有精确边界源时**只填充精确
  区间未覆盖的可执行隙，绝不切分精确区间**（往函数体中间插假起点会劈碎同函数
  共现，是 `fn_*` 回归的头号来源）；无精确源时覆盖全部可执行节（旧启发式的替换
  形态，起点收割语义逐字节一致）。验收测量：notepad.exe 未归属 8→2（**漏检
  5→1**）、trit-re.exe 22→16（**漏检 2→1**），`fn_*` 判定行与基线逐字一致。
  已知边界如实记录：字节模式级不是指令级遍历，数据里的 `E8`/`E9` 会产生假边
  （与旧启发式同风险）；`Cfg` 边界 `exact` 恒为 false，剥表 PE32 的共现**保持
  `Unknown`**——指令级块遍历（真基本块）待最小长度解码器，是 N2 的 v2。
- **最小 CFG v2：x86-64 长度解码器 + 块遍历验证层**（2026-11-05）：
  `x86len.rs` 最小指令长度解码器（fail-closed：VEX/EVEX/三字节逃逸/64 位非法
  指令/moffs 一律 `None` 停走不猜；15 字节上限 + 缓冲区界检查）+ `cfg.rs` 递归
  下降块遍历——种子只取**可信起点**（入口 + 精确区间起点；从字节候选起走会
  拿数据当代码、覆盖面撒谎），起点合成 = 遍历确认的调用目标 ∪ 前向无条件跳
  目标（尾调用归类）∪ 未被遍历覆盖的层一候选（fail-closed 回落到层一，绝不
  静默缩覆盖）。**验收读数与第一层逐字一致**（notepad 2/漏检 1、trit-re.exe
  16/漏检 1）——遍历验证没有改变归属结果（可达调用目标字节扫描本就能看到）；
  v2 的增量是真边验证语义 + 解码器地基；剩余漏检 = 间接调用的叶子（无静态
  `E8` 源），根治在间接调用解析，是 N2 的 v3 方向。测试 114 全绿（+8 解码器）。
- **最小 CFG v3：取址分析**（2026-11-05）：`decode` 新增 `addr_taken`——
  `lea r64,[rip+disp32]` 的落点是**间接调用函数的起点信号**（回调/虚表项）；
  遍历把它们当新种子继续走（不动点），起点合成加第三路。验收实测：
  trit-re.exe 未归属 16→**15**（函数 4259→**4324**，+65 个取址发现的间接
  入口）；notepad.exe 不变——残差漏检是**数据节表项**（vtable 在 `.rdata`，
  代码内 `lea` 够不着），数据节指针表解析（需重定位/节语义）是 v4 范畴。
  `fn_*` 判定全表不变。测试 116 全绿（+2）。
- **最小 CFG v4：数据节指针表收割**（2026-11-05）：`pe_pointer_seeds`——
  非可执行节里指向可执行域的指针串（vtable/回调表）→ 候选起点。机械过滤：
  对齐步进（PE32+ u64 / PE32 u32）、值 − ImageBase 落在可执行域、NULL 跳过、
  **连续 ≥2 全收 + 孤立仅 16 对齐收**（宁可丢也不给假起点）。ELF 诚实留白
  （PIE 需先过重定位）。验收实测：trit-re.exe 未归属 15→**12**（**漏检
  1→0 清零**，函数 +267）、notepad.exe 2 不变（漏检 1 非指针表可解，如实
  保留——不是每条漏检都能机械找回）；`fn_*` 全表不变。测试 118 全绿（+2）。
- **间隙跳板裁定 + 间接调用数据流负结果入册**（2026-11-07）：实测定位
  trit-re.exe 的全部 8 个残余未归属点（全 `jmp`）= 函数间隙里的「`E9 → FF 25`」
  两级跳板链（增量链接独立跳板）；逐点查证——5/8 **无任何引用**（E8/E9/lea/
  imm64 全查无果）、3/8 的引用落在非代码节。**原假设"残余 = 漏检的数据流"
  被否定**：可归因的间接调用早已被跳板展开 + 指针表收割 + 块遍历覆盖，残余
  是死机器，强行解析就是给死代码找主。落地：新裁定 `ThunkGap`（thunk-gap，
  `E9 rel32 → FF 25` 且不在任何函数区间 → 如实单列）；裁定后读数
  trit-re.exe / notepad.exe 的**"疑边界漏检"类别双双清零**（8→0、1→0，
  残余全部如实归入 thunk-gap/clip）。方向库分诊更新：间接调用数据流
  +1 → **0 挂起**（解锁条件：真样本批次出现"疑边界漏检" > 0）。测试 162。
- **PE32 的函数边界不再只剩启发式**（2026-11-03）：没有 `.pdata` 的 PE32（mingw 系默认带
  COFF 符号表）现在从符号表取边界——只认 `Type` 高字节 = `IMAGE_SYM_DTYPE_FUNCTION` 且
  `SectionNumber > 0` 的符号，`Value` 按 RVA 解释并自校验「落在所指**可执行节**内」，
  对不上就丢符号（宁可丢也不给假边界）；区间终点 = 下一个函数起点/所在节末尾。
  权威等级与 ELF `STT_FUNC` 对齐（`FuncSource::CoffSymbols`、`exact = true`），
  于是 PE32 的同函数共现从「一律 `Unknown`」变成可判。
  🔴 证据等级：**合成 PE32 夹具已证**（`tests/pe.rs::coff_symbols_give_pe32_exact_boundaries`
  钉住过滤、逐字节边界、归属与共现可判），**真实 mingw PE32 样本未证**——本机只有
  mingw-w64 x86_64（没有 i686 工具链，`-m32` 编不出来），要按 N1 口径拿到 32 位真样本后补
  `sha256` 记录才算数。

### 4.2 站不住 / 未证的部分

| 问题 | 现状 | 后果 |
|---|---|---|
| 模式匹配不是反汇编 | 只识别固定编码（`FF /2`、`adrp+ldr+blr` 等） | 寄存器经 `mov` 搬运、栈传递的间接调用会漏；数据里的疑似指令可能误报 |
| 无真实 ARM64 样本 | 只有合成夹具 | ARM64 路径的**误报率完全未知** |
| PE32 / 无 `.pdata` | 有 COFF 符号表时边界取自符号表（只有合成夹具证过）；**表被剥掉的 PE32 仍退化启发式** | 剥表 PE32 的共现仍一律 `Unknown`；带表那条路径还没吃过真实 mingw PE32 样本 |
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
| N2 | **最小 CFG**：基本块 + 直接跳转边，替换启发式边界 | ✅ 第一层（2026-11-05）：`unattributed_sites` notepad.exe 8→2 / trit-re.exe 22→16（边界漏检 5→1 / 2→1），`fn_*` 判定与基线逐字一致，启发式边界由 CFG 补隙层替换（`FuncSource::Cfg`；无精确源=全量替换形态，有精确源=补隙不切分）；PE32 共现半只已由 COFF 符号表达成、剥表 PE32 保持 `Unknown`（字节模式级 CFG 不冒充精确——指令级块遍历待最小长度解码器，为 v2） |
| N3 | **接 `trit-procscan`**：现场模块路径 → 台账（✅ 2026-11-03：`trit-re collect` 吃 procscan JSONL 事件流，抽取 `type:"module"` 路径——去重保序、坏行/非 module 事件计数上报、单文件读取失败不中断；每模块产出台账行，末尾一条 `collect-summary`（模块数/去重/不可读/坏行/字节/`elapsed_ms`）——判据「一次采集 ≥ N 条记录」直接从汇总行复算，耗时线性性由两次采集的 bytes/elapsed 对比复算；管道式接入不建 crate 依赖：`trit-procscan … \| trit-re collect`。实测冒烟（真事件流 5 行）：`notepad.exe` 356 KB 分析 22 ms，汇总行给出
`module_events=3/modules=2/deduped=1/analyzed=1/unreadable=1/non_module_events=1/bad_lines=1/bytes=356352`，
stdout 622 行（621 条台账 + 1 条汇总）；路径写成 JSON 非法转义时 `module_events=0/bad_lines=3` 一条不解析但计数照报。
测试 `tests/collect.rs` 4 条用内存文件系统） | 一次采集产出 ≥ N 条 `xref`/`function` 记录，耗时与文件大小线性 |
| N4 | **YARA 子集的三值匹配** | ✅ 两层（2026-11-05）：第一层 `sigscan.rs`——hex 串（`??` 通配 + `[n]`/`[n-m]` 跳跃、跳跃后必须有字节段）、文本串（ascii/nocase ASCII 折叠/wide=仅 UTF-16LE）、声明式条件 + **表达式树**（And/Or/Not/Ref/OfAny/OfAll/OfCount）；三值纪律与验收判据逐字对应——**命中 True（不依赖完整性）、完整扫描未命中 False、输入截断 Unknown**（"未命中"只在完整扫描后给出），整体 = Kleene OR（报警语义：`False ∨ Unknown = Unknown`，截断的沉默不是清白）；护栏：每模式命中 ≤1024、跳跃 ≤0x1000。第二层 `yaraparse.rs`——`.yar` 规则文件解析器（词法器引号感知——字符串里的 `//` 是内容不是注释；meta 计数、`import` 入 notes；字符串修饰符 nocase/wide/ascii；hex 块；条件递归下降：and/or/not/括号/`$ref`/`at`/`in`/`any·all·N of them/($x*)`）；**解析不了的规则 → `Unsupported` 带原因 → 扫描时 `Unknown`，绝不静默跳过**（正则串、filesize/pe.*、未知 `$ref` 等 8 类端到端钉死）；**对拍基建已就位 + 真对拍已跑通**（2026-11-05）：`trit-re sigscan <样本> --rules <.yar>` CLI（HIT/MISS/UNKNOWN/UNSUPPORTED 逐规则一行，对拍口径）+ `scripts/yara_diff.py`（官方引擎 vs trit-re 四态分类：MATCH/DIFF/SUBSET-GAP/OFFICIAL-ERR，真 DIFF 退出码 1）；环境通路 = uv（`uv python install 3.13` + `uv pip install yara-python`，免 MSVC）；**真对拍结果：合成对抗批次 6 样本 × 9 规则（官方 YARA 4.5.4）MATCH 6 / DIFF 0 / SUBSET-GAP 4**——可评估规则命中集合 100% 一致，正则/filesize 规则正确落入子集边界类；对拍还揪出一个词法 bug（`.` 不在断字集，`in (4..8)` 挂——已修）；**加固轮**：`SCAN_BUDGET`(1M 步/模式) 封顶跳跃回溯的指数爆炸（对抗数据上 `{90 [0-4096] …}×全0x90` 会挂起）——耗尽 = 扫描不完整 = 裁决 `Unknown`（"未命中只在完整扫描后给出"的组成面）；确定性模糊 `tests/fuzz_sigscan.rs` 五路夹击（随机文本/结构变异/随机 spec/随机数据×模式/对抗跳跃链）不变量 = 不 panic 不挂起；🔴 剩余：真实样本批次上的规模化对拍（harness 就绪，喂样本目录即跑）；**完整子集轮（2026-11-07）**：条件求值升级三值 Kleene——`Cmp`（filesize/uintN/#a/@a/字面量，KB·MB 后缀）、`ForOf`（N\|any\|all，体内 `$`/`#`/`@` 绑定当前串，体弃权不折叠成假）、`entrypoint` 与 `pe.*`/`math.*` 记名弃权（求值恒 Unknown）；正则/base64/private 串可解析不可扫描（`Unscannable` 带原因，整条规则 Unknown、其余串照常扫）；`xor`/`xor(k)`/`xor(a-b)`、`fullword`（wide 词边界）、`ascii wide` 双变体可评估；无 strings 纯数值规则合法；模块拆分守 800 行预算（`sigscan/matchers.rs`、`sigscan/tests.rs`、`yaraparse/cond.rs`）；yara 集成测试 9 → 18；xor+wide 组合语义未与官方逐位对拍（等校准批次）；**entrypoint 接线轮（2026-11-07）**：`entrypoint` = 入口点**文件偏移**——官方语义 yara-python 实测判定（RVA≠offset 的两个系统 DLL 上官方只认 offset，不猜）；CLI `pe::parse` 的 `entry_rva` 过 `rva_to_offset` 传入 `scan_with_entrypoint`，库直调不传上下文时弃权；ELF 侧如实留白（无样本验证语义）；**`$a at X` 语义修正首次命中 → 任一命中**（官方 `at` 语义——真 PE 烟测揪出：高频序言在入口前命中 1753 次会误判 MISS）；**`MAX_HITS` 1024 → 65536 且封顶截断 = 扫描不完整**——位置性条件据残表如实弃权绝不判假（"截断的沉默不是清白"落到命中表）；真 PE 烟测 `at entrypoint` HIT 与官方一致；测试 161；**差分测试规模化轮（2026-11-07，论文 E5）**：`scripts/yara_scale.py` 属性驱动差分测试（xorshift64 确定性种子 × 随机样本，复用 yara_diff 四态）——**再抓两个真语义偏差**：①`for-of` 迭代**整个串集**（官方：未命中串也在迭代里 `#`=0；`for all of … : (# > 2)` 惯用法依赖此——实证针缺席时官方 `for any … : (# >= 0)` 仍命中）②**确定的未定义 ≠ 证据缺失**（零命中 `@a[N]`、非截断越界 uintN → 比较为假；截断/缺上下文 → 弃权——resolve 三态）；**xor 组合校准**：官方拒绝 `xor nocase`（探针实测）→ 解析层对齐拒绝；`xor wide` 正反例逐位一致（最后已知边界清零）；**规模曲线：10 批 5280 判定对 MATCH 100% / DIFF 0 / SUBSET-GAP 0**；测试 165 |
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
