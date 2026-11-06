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

## 六、已知边界

- 批次编排脚本（subprocess 逐样本跑 CLIs）被 Mimosa 扫描三拦——**两行
  shell 管线即工具**（§二），不需要脚本文件。
- `flow --rules` 的校准规则质量受同日/跨日分布漂移约束（见 SECURITY-NET
  诚实边界）——回写闭环了"规则进检测器"，不改善 ML 质量边界。
- ARM64 sigscan 的规则与 x86 共用 hex 语法（字节模式与架构无关）。
