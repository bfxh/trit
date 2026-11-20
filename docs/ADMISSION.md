# 入库准入：什么能进版本库（**强制**）

> 一句话：**提交前必须先想清楚「这东西是什么、凭什么进版本库」**。
> 想不清楚就不要放进去——放进去之后，代价是仓库膨胀、CI 额度烧穿、门被跳过、历史要重写。
>
> 执行者：`scripts/repo_admission.py`（pre-commit 第 0 步，默认开）。
> 策略表看一眼：`python scripts/repo_admission.py --explain`。

## 1. 三档判定

| 档位 | 含义 | 例子 | 退出码 |
|---|---|---|---|
| **BLOCK** | 无论写什么理由都不许入库 | 编译产物、模型权重、归档/备份、日志/数据库、密钥文件与内容、单文件 > 2 MiB、提交信息里指向 AI 厂商的 trailer | 1 |
| **REASON** | 必须写「入库理由:」才能过 | 二进制资产 > 0.5 MiB、任何 > 0.5 MiB 的文件、单次新增 > 20 个文件 | 1（没写理由时） |
| **FREE** | 直接入库 | 源码、文档、配置、小型文本夹具 | 0 |

「入库理由」写在**提交信息**里，一行，值 ≥ 10 字：

```
docs: 加入 47 KB 黄金语料夹具

入库理由: 这是解析器的规范输入，无法由源码生成，改判据时用它做 DIFF
```

英文字段名 `Why-in-repo:` 同样有效。

## 2. 能接受什么构建产物（用户口径，2026-11-21）

| 类别 | 具体 | 判定 | 条件 |
|---|---|---|---|
| 源码 / 文档 / 配置 | `.rs .py .md .toml .json .yml` 等 | FREE | 无 |
| 小型文本夹具 / golden 数据 | `tests/fixtures/*.txt`、`interop/vectors/*.json` | FREE | ≤ 0.5 MiB |
| 上游补丁原文 | `crates/*/upstream/patches/*.patch` | allow | 必须在 `.agent/admission.json` 写 reason；本项目已登记 3 个 |
| 内置最小模型夹具 | `crates/trit-parts/models/*.tpm`（467 KiB） | allow | 同上：基准的固定输入，无法由源码生成 |
| 文档资产 | 图片 / 字体 / PDF | ≤ 0.5 MiB FREE；更大需理由；> 2 MiB 拦 | 图标 SVG 直接 FREE |
| 大块数据 / 语料 | 任何 > 0.5 MiB 文本 | REASON | 写清是什么、为什么不能生成、怎么复算 |
| 编译产物 | `target/`、`*.rlib *.so *.dll *.exe *.wasm *.pyc` | **BLOCK** | 永不 |
| 模型权重 / 列存 | `*.safetensors *.gguf *.onnx *.pt *.npy *.parquet` | **BLOCK** | 走发布包或外部存储，仓库只存指针与校验和 |
| 归档 / 备份 / 补丁备份 | `*.bundle *.zip *.tar.* *.7z *.orig *.rej *.patch（非上游补丁）` | **BLOCK** | 放归档根目录（见 `docs/COLLAB.md §10`） |
| 日志 / 转储 / 本地数据库 | `*.log *.dmp *.sqlite *.db` | **BLOCK** | 它们是运行期状态 |
| 密钥 / 凭据 | `.env`、`id_rsa*`、`*.pem *.key *.p12`、高置信度 token | **BLOCK** | 泄露后必须轮换 + 重写历史 |
| CI 产物 | Actions artifact zip、coverage 报告 | **BLOCK** | 走 Actions 的 artifact，不进仓库 |
| 本机落盘策略 | `.cargo/config.toml`、`.agent/memory/`、`.trit/`、`runs/` | **BLOCK** | `.gitignore` 已覆盖，别 `-f` |

不在表里的东西：**默认按 REASON 处理**——先想清楚，再决定。

## 3. 安全问题 → 连锁后果（这就是为什么要强制）

| # | 触发 | 直接后果 | 连锁后果 | 现在的拦截 |
|---|---|---|---|---|
| 1 | 把编译产物 / 生成物入库 | 仓库持续膨胀，`clone` 与 CI checkout 变慢 | CI 分钟额度更早烧完 → 排队 → 智能体等不起 → `--no-verify` 跳过门 → 脏东西进 `main` | pre-commit 第 0 步 BLOCK；`.gitignore` 覆盖 `target/`、`runs/`、`.trit/` |
| 2 | 把权重 / 单个 > 100 MiB 文件入库 | GitHub 硬拒 push（单文件上限） | 为了推上去强推或重写历史 → 别人的 `worktree` 全部失效 → 未推送的提交与 `stash` 丢失（§10 的丢失链） | > 2 MiB 直接 BLOCK，权重后缀单独 BLOCK |
| 3 | 密钥入库 | GitHub push protection 拦截；一旦真的推上去就视为泄露 | 必须轮换 + `git filter-repo` 重写历史 → 所有 `worktree` 与本地分支失效 → 又一轮工作丢失 | 密钥文件名 + 高置信度内容 BLOCK；`secret_scan.py` 在 CI 复跑 |
| 4 | AI 厂商账号/身份进 commit | 违反归属规则（版权必须是用户本人） | CI 拦 → 需要重写历史 → 与 #3 同样的连锁 | pre-commit 查提交信息 trailer；`pre-push` 的版权护栏 v2 再查一遍 author/committer/remote |
| 5 | 会话额度耗尽被中断 | 未提交改动留在目录，`stash` 不留引用 | 目录被清理 → 分支成孤儿、`stash` 消失 → `gc` 后永久丢失 | `docs/COLLAB.md §10` 三条硬规则 + `archive-worktrees.sh` / `rescue-dangling.sh` |
| 6 | 习惯性 `--no-verify` | 本地门形同虚设 | 问题只在 CI 暴露 → 排队 → 为省时间再次跳过 → 循环 | `pre-push` 记录绕过；本闸门属"默认开"，关掉也会打印风险 |
| 7 | 本机状态 / 日志入库 | 覆盖别人 `worktree` 的配置与状态 | 环境不一致 → 「我这儿能跑」→ 排查成本转嫁给别人 | `.gitignore` 排除 `.agent/memory/`、`.cargo/config.toml`、`.agent/admission.log` |
| 8 | 归档 patch / bundle 入库 | 与真实分支分叉，恢复时打错版本 | 「恢复成功」但恢复到废弃实现上，浪费一整轮 | 归档类后缀 BLOCK，恢复入口统一在归档根目录（`docs/COLLAB.md §10`） |
| 9 | 批量抓数据不写理由 | 没人知道它是什么、能不能删 | 长期滞留 → 每次 `clone` 背负它 → 想删又怕删错 | 批量新增 > 20 个文件要求「入库理由:」 |

**共同点**：每一个都不是"多占几 MB"的问题，而是**把后续所有门都拖垮**。

## 4. 开关：用户可以自己关，但关了也会被提醒

优先级（任一即关闭）：`TRIT_ADMISSION=off` → `git config trit.admission off` → `.agent/admission.json` 的 `"enabled": false`。

关闭后的行为**不是静默放行**：

```
[admission] 已关闭（环境变量 TRIT_ADMISSION=off）——仍然检查，只是不拦。
[admission] 风险 target/debug/a.rlib（0 KiB，构建产物目录）
            → 构建产物不入库：改 .gitignore，或把它留在归档根目录（docs/COLLAB.md §10）
[admission] 可能的原因与连锁后果：
  - 编译产物/权重/归档入库 → 仓库膨胀 → clone 与 CI 拉取变慢 → CI 额度更早烧完 → 门被跳过 → 脏产物进 main
  - 密钥入库 → push protection 拦截或真泄露 → 必须轮换 + 重写历史 → 所有 worktree 失效 → 未推送工作丢失
  - 本机状态/日志入库 → 别人的 worktree 被覆盖 → 复现环境不一致
[admission] 重新开启：.agent/admission.json 里 "enabled": true，或去掉 TRIT_ADMISSION=off
```

同时写一行审计到 `.agent/admission.log`（不入库）。

**为什么要这样设计**：门被关掉时，最需要的是"可能是哪一类问题"的提示，而不是安静地放行——
不然下次发现仓库变大，没人知道是从哪一次提交开始的。

## 5. 豁免：必须写理由

```json
// .agent/admission.json
{
  "enabled": true,
  "max_mb": 2.0,
  "reason_mb": 0.5,
  "bulk_files": 20,
  "allow": [
    { "path": "crates/trit-immune/upstream/patches/*.patch",
      "reason": "上游 crate 的适配补丁：下游复现与审计需要原文，体积 0-2 KiB" }
  ]
}
```

`reason` 空白的条目会被**忽略并告警**——写不出理由，就说明它不该入库。

## 6. 命令

```bash
python scripts/repo_admission.py                 # 查已 staged 的文件（pre-commit 用）
python scripts/repo_admission.py --all           # 体检全部被跟踪文件（存量只报告，不拦）
python scripts/repo_admission.py --explain       # 打印策略表
python scripts/repo_admission.py --self-test     # 正例 + 逐条反证
bash scripts/install-hooks.sh                    # 让 pre-commit 生效（每个仓库/镜像一次，worktree 共用）
```

## 7. 与其它门的关系

| 门 | 管什么 | 与本闸门的分工 |
|---|---|---|
| `.agent/admission.json` + `repo_admission.py` | **什么能进版本库** | 本文件的主题 |
| `scripts/secret_scan.py` | 密钥（全仓 + 历史） | 本闸门在提交时先挡一次，CI 再全仓复扫 |
| `scripts/ai_vendor_scan.py` / `pre-push` 护栏 v2 | AI 厂商身份与归属 | 本闸门查提交信息 trailer，pre-push 查 author/committer/remote |
| `scripts/module_budget.py` | 单文件行数 | 只管源文件胖瘦，不管产物 |
| `docs/COLLAB.md §10` | 工作不丢失 | 管"已做的工作别丢"，本文件管"进来的东西别脏" |
