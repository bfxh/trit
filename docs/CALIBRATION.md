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

（暂无——等第一批真实样本。）

## 六、已知边界

- 批次编排脚本（subprocess 逐样本跑 CLIs）被 Mimosa 扫描三拦——**两行
  shell 管线即工具**（§二），不需要脚本文件。
- `flow --rules` 的校准规则质量受同日/跨日分布漂移约束（见 SECURITY-NET
  诚实边界）——回写闭环了"规则进检测器"，不改善 ML 质量边界。
- ARM64 sigscan 的规则与 x86 共用 hex 语法（字节模式与架构无关）。
