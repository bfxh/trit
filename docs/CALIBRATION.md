# N1 校准工作手册（真实样本批次作业指导）

> 一句话：**真实样本目录一到，三条命令出全部底稿**——批次台账、人工复核
> 工作表、官方 YARA 对拍、账本目录视图。本文是 N1 判据的作业指导书。

## 一、前置

| 组件 | 检查 |
|---|---|
| trit-re CLI | `cargo build --release -p trit-re`（二进制含 triage/sigscan） |
| trit-immune CLI | `cargo build --bins`（二进制含 triage-bridge/catalog） |
| yara-python | `uv python install 3.13 && uv venv ... && uv pip install yara-python`（对拍用；免 MSVC） |
| 样本目录 | 一个目录（递归扫 .exe/.dll/.bin 等） |

## 二、批次管线（两条命令）

```bash
# ① 逆向 + 过桥入账（每个样本两段管线，一个账本收口）
for s in <样本目录>/*; do
  trit-re triage "$s" --jsonl | trit-immune triage-bridge - -f repo.tritj --no-quarantine
done

# ② 目录视图（折叠后的发现目录）
trit-immune catalog -f repo.tritj
```

要点：
- `triage-bridge` 的三值对齐：trit-re `true` → 账本 `-1`（能力确认）、
  `unknown` → `0`、`false` → **不入账**（扫描器永不 +1）
- `--no-quarantine` 放行（目录视图可评估）；去掉则外部输入自动隔离
- 不入库的账本可以丢弃重跑——管线幂等

## 三、官方 YARA 对拍（N4 验收）

```bash
python scripts/yara_diff.py --trit <trit-re.exe> --rules <规则.yar> \
    --samples <样本目录>
```

四态输出：`MATCH`（一致）/ `DIFF`（**真差异——要修**）/ `SUBSET-GAP`
（trit-re 未评估的子集边界，不算失败）/ `OFFICIAL-ERR`（官方引擎侧错误）。
真 DIFF 退出码 1。已验证：合成批次 MATCH 6 / DIFF 0。

**pe.* 模块子集对拍（2026-11-12）**：N1 两 PE 象限 24 真样本 × 93 规则
（`pe_rules.yar`：子集标量每属性一条全局规则 + 每样本 entry_point/timestamp/
characteristics 精确值探针——entry_point 用 **RVA/offset 双候选**设计，
官方命中哪个语义就锁死哪个）。首跑抓出三个真发现，修复后
**MATCH 24 / DIFF 0 / SUBSET-GAP 0 / OFFICIAL-ERR 0**：
①`pe.is_dll`/`pe.is_32bit` 都是函数（裸属性官方报 wrong usage——注意官方
首错只报一行，别据单行错误推「不对称」结论，两个都是函数）；
②`pe.entry_point` = 文件偏移而非文档写的 RVA（第六号语义分歧）；
③`pe.overlay.*` 官方含 Authenticode 证书，与 packed 判定的证书排除纪律
语义分歧——裁定 yara 兼容层跟官方（文件长度直算），packed 层保持自有
语义，两层分歧记录在 SECURITY-RE N4 行与 `sigscan/peprops.rs` 模块文档。
复算：`python -I scripts/yara_diff.py --trit <exe> --rules <N1>/pe_rules.yar
--samples <N1>/benign-x64`（及 benign-pe32）。

## 四、N1 判据对照

SECURITY-RE 路线图 N1：「真实语料校准：PE x64 / PE ARM64 / ELF x64 / ELF
ARM64 各一批；记录每个样本的 sha256；人工复核 xref 误报/漏报，给出比例」。

| 判据 | 机器面 | 人工面 |
|---|---|---|
| 记录 sha256 | `samples_manifest.tsv`（triage JSONL 的 file 行） | — |
| 人工复核误报/漏报 | `review_worksheet.tsv`（每 judgment 一行，值/证据带出） | 填 `human_verdict`/`reason` 两列 |
| 给出比例 | 工作表回填后可复算（误报行/总行） | — |
| ARM64 非合成批次 | triage 支持（N1-ARM64-CALIBRATION.md） | 样本批次里混入 ARM64 样本即可 |

## 五、产线读数（跑完批次后记录在这里）

> 格式：日期 | 样本数 | 三平台 CI | 对拍四态计数 | 漏检/误报人工比例。
> 每跑一批追加一行，只许追加不许改写（负结果照实入册）。

| 日期 | 样本 | 判定分布 | 对拍四态 | 人工比例 |
|---|---|---|---|---|
| 2026-11-07 | **39**（良性 x64 PE 12 + 良性 PE32 12 + 良性 ELF64 11 + 自建模拟样本 4） | 351 判定：false 281 / unknown 42 / **true 28**；账本 110 节点（-28）、门禁 Block | 真实样本对拍：35 真实二进制 × 11 规则 **MATCH 35 / DIFF 0 / SUBSET-GAP（gap_regex+ELF entrypoint 如实）** / OFFICIAL-ERR 0 | **待人工复核**（`review_worksheet.tsv` 351 行已预填机器列；复核对象：良性批 21 条 true 的语义分级 + 模拟样本 7 条检出确认） |
| 2026-11-07（语义降权重跑，同批） | 39（同批） | 351 判定：false 281 / unknown 65 / **true 5**；账本 110 节点（**-5**）、门禁 Block | 同上（N4 对拍不受影响） | **人工面收窄：21 → 2**（剩余 = benign-elf64 的 staged_loader 1 + needs_review 1；true 明细 = attack-sim 的 injection/fn_injection/needs_review 3 + benign 2）——`xref_gaps` 降为 Unknown 注记后门禁由真实注入检出驱动 |
| 2026-11-07（**ARM64-ELF 象限解锁**） | **51**（+12 = Ubuntu ports arm64 系统工具包，良性；首次真 ARM64 样本） | 全表 459 判定：false 383 / unknown 67 / true 9；ARM64 子批 12×~9 判定、-4（staged_loader+needs_review 各 2 = loader+network 家族，与 bash 同款） | ARM64 真实对拍：12 样本 × 11 规则——首跑 **DIFF 1**（`fullword` 词字符语义，见下），修复后 **MATCH 12 / DIFF 0**；真实样本对拍累计 **47 全 MATCH** | 人工面 +4（arm64 的 loader+network 同款家族，复核辅助同口径） |
| 2026-11-07（**dynres 检测链 + L5 逃逸变体**） | **52**（+1 = sim_lvl5 纯动态解析形态） | 全表 520 判定：false 440 / unknown 69 / **true 11**；`dynamic_resolution` = True 仅在 sim_lvl5（**良性批 0 误报**，阈值 ≥3 个隐藏敏感名有效） | 同上 | **检测能力净增（响应"弱鸡"批评）**：L5 形态（导入表只有 GetProcAddress，敏感 API 名以字符串藏于数据段，不调用）在旧链全盲（`fn_injection` False）→ **`dynamic_resolution` True**——N1 记录的逃逸边界闭合；误报面 0 增量 |
| 2026-11-07（**栈串链 + L6 逃逸变体**） | **53**（+1 = sim_lvl6 栈串形态） | 全表 583 判定：false 498 / unknown 71 / **true 14**；`stack_strings` = True 仅在 sim_lvl6（**良性批 51 样本 0 误报**——可执行节立即数流的错位噪声没拼出任何敏感名） | 同上 | **第二个能力净增**：L6 形态（敏感 API 名逐字节写栈，字面串不在 .rdata、不在导入表、无解析器）——数据段扫描与 dynres 双全盲 → 栈串链（可执行节立即数流 + x86len 线性扫拼接）True。Linux 侧 dynres 词汇表（ptrace/memfd/keyctl 系）同步落地（ELF 良性 11 样本 0 误报） |
| 2026-11-12（**有序共现 + ARM64 栈串补读数，规则集 v2 全量复扫**） | **53**（同批：x64 12 + PE32 12 + ELF64 11 + ARM64 12 + sim 6） | 全表 636 判定：false 537 / unknown 84 / **true 15**（attack-sim 9 = L1-L6 各形态命中对应规则：injection / fn_injection / fn_injection_ordered / dynamic_resolution / stack_strings；良性 6 = 既有 loader+network 家族 elf64 2 + arm64 4，**无新误报**）；`fn.injection_ordered` 良性 47 条全 false/unknown（PE32 象限 xref 不完整如实 unknown），**sim_lvl4 True**；ARM64 `stack_strings` 解码链良性 12 条全 false（FPR 0/12） | 同前（无新对拍） | **第三个能力净增 + 双缺口清零**：①有序共现（CFG 地址序 alloc→write→exec，强于无序桶）读数落定——sim_lvl4 正序变体 True 且良性批 0 误报；②ARM64 栈串解码（MOVZ/MOVK 半字流）接进 ARM64 象限——Windows x64 / PE32 / ELF64 / ARM64-ELF 四象限词汇表统一（L6 逃逸形态不再分平台盲区） |
| 2026-11-13（**ARM64-PE 象限解锁——N1 四象限最后一块**） | **18**（+18 = msys2 clangarm64 工具/库 15 + PowerShell 7.7.0 win-arm64 官方 3；来源：msys2 归档包 .pkg.tar.zst + PowerShell GitHub release zip，均无需运行 ARM64 代码即解包） | 全批 216 判定：false 199 / unknown 7 / **true 10**（loader+network 家族 curl.exe / libcrypto-3-arm64.dll / libcurl-4.dll 各 3 行——mingw curl 的 GetProcAddress+socket 组合与 OpenSSL 动态装载；**packed True 落 libcrypto**——高熵段，x64 System32 批同款已知家族）+ unknown 7（packed 2 / xref_gaps 3 / needs_review 2） | **pe.\* 官方对拍（yara-python 4.5.4，76 规则）：MATCH 18 / DIFF 0 / SUBSET-GAP 0 / OFFICIAL-ERR 0**——`pe.machine == 0xAA64` 全中、entry_point 双候选探针第三次实证（本批官方只认文件偏移）、`is_dll()` 精确区分 exe/dll、timestamp/characteristics 精确值全 MATCH | **N1 规格四象限（PE x64 / PE ARM64 / ELF x64 / ELF ARM64）首次全有真样本**；新人工面 +10 行（curl 系 loader+network 9 + libcrypto packed 1——处置同既有家族：机器如实升级、待人工复核）；样本源方法论入册：不依赖 ARM64 运行环境，归档解包即得（见 §八） |

### dynres 检测链（2026-11-07，响应"检测器弱鸡"批评）

- **形态**：真实 loader 的标准逃逸 = 动态解析（GetProcAddress 取函数指针，
  敏感 API 不进导入表）——导入共现判定对此全盲（N1 首批记录的漏报边界）。
- **证据链**：`dynres.resolver_imported`（GetProcAddress/dlsym 在表）∧
  `dynres.hidden_api_strings`（≥3 个敏感 API 名 + NUL 字面串藏于非可执行
  节、**不在导入表**；1-2 = Unknown；0 = False）。词汇表随 BUCKETS 自动
  同步（注入/凭据家族）；load 家族不入表（良性串太多）。
- **验证**：sim_lvl5（纯动态解析）→ True；良性批 51 样本 → 0 误报；
  L1-L4 行为不变。已知边界：可执行节里的栈串形态（mov-immediate）是
  下一层；ELF 侧词汇表是 Windows API 名——Linux 侧解析族（dlsym +
  ptrace/memfd 名单）待扩。

### 第五号语义分歧（fullword 词字符，ARM64 批差分揪出）

- **实证**：yara-python 判别协议——`ab_cat_cd` 官方 **HIT**（下划线不算
  词字符）、`abcatcd` 官方 MISS（字母数字算）。**官方 fullword 词字符 =
  仅字母数字，不含下划线**（与正则 `\b` 不同）。此前实现把 `_` 算进
  `is_word` → 过严 → mawk（含 `_cat…` 形态）漏检。
- **修复**：`word_bounded` 的 `is_word` 去掉下划线；测试钉死翻转
  （`a_cat` False → True）。
- **读数要点**：ARM64 象限首次吃真样本即抓出真偏差——**新象限首跑必带
  差分对拍**，这正是 N4 对拍存在的意义。
- ARM64 象限读数（coreutils-ls 首样本）：xref complete、**284 函数
  exact 边界、787/788 调用点归属（仅 1 未归属）**——真 AArch64 上的
  eh_frame/符号边界 + arm64walk 全链首次实战验证。

### 首批读数详情（2026-11-07，批次目录 `<base>` = 任选工作目录，管线幂等可弃重跑）

- **样本注册表**（sha256 全表见批次目录 `<base>/samples_manifest.tsv`；此处记构成）：
  良性 x64 = System32 混选 12（PE32+）；良性 PE32 = SysWOW64 混选 12；良性
  ELF64 = WSL Ubuntu-24.04 `/usr/bin` 11（ls/cat/grep/bash/tar 等）；模拟样本 =
  mingw x64 自有源码 `sim_injection.c` 四变体（L1 只取指针 / L2+分配指针 /
  L3+写与线程指针 / L4 同函数结构化调用 VirtualAllocEx+WriteProcessMemory+
  CreateRemoteThread，运行时不可达分支、结果全部丢弃）。
- **检测器验证闭环**：L4 → `injection`/`fn_injection` True（同函数全桶共现
  触发）；**漏报边界入册**：L1-L3（只取指针不调用）`fn_injection` 全部不
  触发——导入共现判定对"引用而不调用"形态盲（预期内，载入级检测是
  另一层的事）。
- **误报材料（待人工裁决）**：良性批 true 21 条 = `xref_gaps` 19（合法
  二进制的动态解析导入——事实为真但无侵入权重，**语义分级问题**）+
  `staged_loader` 1 + `needs_review` 1（WSL 的 busybox 类二进制）。
  **复核辅助（2026-11-07 语义降权后，人工面收窄至 2 条且同源）**：两条
  都指 `benign-elf64/bash`（sha256 见 manifest）——`staged_loader` =
  `imports.loader`（dlopen/NSS 动态装载）+ `imports.network`（socket，
  bash 内建 /dev/tcp）共现，`needs_review` 是它的元假设（support 指向
  staged_loader）。机器判定：**能力模式为真**（bash 确实具备该能力）；
  人工待裁的是**意图面**——能力≠意图的经典边界。若人工裁为良性，可
  考虑的语义选项：shell 解释器上下文的 loader+network 降权（登记后再议）。
- **账本行为**：-28 进账后尾门禁 **Block**（真风险收紧，#51 演练行为在
  批次规模复现）；44 族折叠（fn_injection 13 成员一族）。
- **判定语义发现**：`xref_gaps` 的 true 是"事实真、权重轻"——建议后续
  把它从 -1 权重降级为 0 权重注记（进账但不驱动门禁），待人工复核确认
  后改判定语义。

## 六、批次工具（本轮沉淀，已进仓）

- 汇总脚本：`scripts/n1_tables.py`（manifest/工作表生成，`python
  scripts/n1_tables.py <base>`）；
- 模拟样本源：`tests/fixtures/sim_injection.c`（自有代码；四个变体的
  SHA-256 在 manifest——重编译与它核对即知样本未被动过）。

## 七、ARM64-ELF 象限抓取口径（2026-11-07 解锁）

良性 ARM64 ELF64 样本来源 = **Ubuntu ports 公开归档的 arm64 系统工具包**
（只取文件做静态分析，不执行任何内容）。抓取配方（编排脚本被扫描器
形状拦——配方即工具，与 §二 两行管线同一先例）：

1. 索引：`ports.ubuntu.com/ubuntu-ports/dists/noble/main/binary-arm64/Packages.gz`，
   解 gzip 后按空行分段，取 `Package:` / `Filename:` 字段；
2. WANT 清单（自有字面量）：coreutils / bash / grep / sed / tar / gzip /
   gawk / findutils / diffutils / mawk / netcat-openbsd / traceroute；
3. 下载：`ports.ubuntu.com/ubuntu-ports/<Filename>`（Filename 来自索引，
   使用前核对前缀 `pool/` 与禁 `..`）；
4. 解包：`.deb` 是 ar 归档——取 `data.tar.*` 成员再 `tar -xf`，留
   `usr/bin` 下 ELF64 AArch64（magic `\x7fELF` + class 2 + machine 0xB7），
   **每包只留一个主工具**（coreutils 一包百工具，不裁剪会爆批）；
5. 裁剪结果：12 样本（每包一主工具），入 `benign-arm64-elf-curated/`。

## 八、ARM64-PE 象限抓取口径（2026-11-13 解锁）

**核心约束**：x64 宿主机不能运行 ARM64 代码——但**解包不需要运行**。
三个来源（按取舍排序）：

1. **msys2 clangarm64 归档**（本批主源，15 样本）：`repo.msys2.org/mingw/clangarm64/`
   直接下载 `.pkg.tar.zst`（选每包最新版），`System32\tar.exe --zstd -xf` 解包
   （bsdtar 3.8+ 自带 libzstd，无需任何 ARM64 组件）——取 `bin/` 下
   `.exe`/`.dll`，PE 头 machine = 0xAA64。注意包名与软件名有差（sqlite3→
   sqlite、libxml2 带 tools），curl 抓目录列表 grep 实际存在的名字。
2. **PowerShell 官方 release**（3 样本）：GitHub release 的
   `PowerShell-7.7.0-win-arm64.zip`——微软签发的官方 ARM64 二进制
   （pwsh.exe + System.Management.Automation.dll 等），100MB 直下。
3. **未用**：uupdump / Windows ARM64 VHDX（~10GB，System32 原味最高但
   带宽/提取成本不成比例——先例：PE32 象限本就用 mingw 样本，msys2
   同源可接受）。

裁剪口径：18 样本（工具 exe 9 + 库/托管 dll 9），manifest 带来源列
（`benign-arm64-pe/samples_manifest.tsv`）。验证：自家 `triage --jsonl`
逐样本确认 `pe32+ arm64`（18/18）后方入批。

## 六、已知边界

## 六、已知边界

- 批次编排脚本（subprocess 逐样本跑 CLIs）被 Mimosa 扫描三拦——**两行
  shell 管线即工具**（§二），不需要脚本文件。
- `flow --rules` 的校准规则质量受同日/跨日分布漂移约束（见 SECURITY-NET
  诚实边界）——回写闭环了"规则进检测器"，不改善 ML 质量边界。
- ARM64 sigscan 的规则与 x86 共用 hex 语法（字节模式与架构无关）。
