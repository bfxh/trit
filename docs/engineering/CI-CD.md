# CI/CD 与发布流程

> 状态：已落地（2026-11-23，质量门入口见 `scripts/agent_quality_gate.py`）。本文只描述**开发侧质量门与发布链**，不是用户手册。
> 相关：[WORK-BENCH.md](WORK-BENCH.md)（工作任务基准）、[CMD-DRIVER.md](CMD-DRIVER.md)
> （PTC 命令行降错）、[WORKTREES.md](WORKTREES.md)（发布 preflight 的 worktree 口径）。

## 1. 两层边界：开发内部 vs 用户交付

| 层 | 内容 | 谁能看到 | 是否进入 release assets |
|---|---|---|---|
| 开发内部 | `work_bench` / `cmd_driver` 命令、自测细节、错误分类、PTC receipt、CI job 名、fixture 路径、内部日志 | 仓库维护者、CI 运行页 | **否** |
| 用户交付 | `trit` 三平台二进制归档、`SHA256SUMS`、GitHub Release 说明、用户文档 | 最终用户 | **是** |

硬规则：

- 发布资产只允许 `trit-linux-x86_64.tar.gz`、`trit-windows-x86_64.zip`、
  `trit-macos-aarch64.tar.gz` 与 `SHA256SUMS`；`release.yml` 用 `pattern: trit-*`
  下载构建产物，`agent-quality-evidence` 不在该模式内。
- `agent-quality` 的 gate JSON、`ptc-receipt.json`、`work-bench-report.json` 只作为
  CI artifact 保留 7 天，供维护者排障；不写进 Release，不贴进用户 release notes。
- Release 说明由 GitHub 生成，面向用户；不要把内部 job 名、命令行参数、receipt 路径
  或测试夹具写进说明。需要解释质量时，只写“质量门通过”这类结论。

## 2. Workflow 分工

| 工作流 | 触发 | 作用 | 是否用户可见 |
|---|---|---|---|
| `ci.yml` | push / PR | Rust fmt、clippy、test、registry、preflight self-test、hazard、package、wasm 等仓库级检查 | 否 |
| `agent-quality.yml` | PR 到 `main` 且触及 work-bench / cmd-driver / selftest / 夹具；手动 dispatch | 在 Ubuntu + Windows 跑 `agent_quality_gate.py`，保留 receipt 与报告 | 否 |
| `release.yml` | push tag `v*`；手动 dispatch | `guard` 校验 tag=版本 → `agent-quality` 质量门 → 三平台 build/test/smoke/打包 → 生成校验和并发布 | 只发布资产与说明 |

`agent-quality.yml` 用路径过滤而不是全量触发：它依赖 `openpyxl` 安装，且只在
work-bench / PTC 相关文件变化时才需要重跑。全仓库 Rust 测试仍由 `ci.yml` 的
`check` 与 `release.yml` 的 host test 覆盖。

## 3. 统一质量门

本地与 CI 使用同一条命令：

```bash
python scripts/agent_quality_gate.py --out target/agent-quality/agent-quality-gate.json
```

它做两件事，不另造评分层：

1. `python tools/cmd_driver.py -SelfTest`；
2. `python tools/cmd_driver.py -Plan testdata/work-bench/ptc-plan.json -Out <out>/ptc-receipt.json`。

第二步的 PTC plan 覆盖：

- `preflight`：Python + `openpyxl` 可用；
- `work_bench.py -SelfTest`：182/182；
- `work_bench.py -Run`：20 任务、81/81 reference、20/20 negative；
- `work_bench.py -Run -Repeat 2`：重复稳定。

门只看 receipt 与两份 work-bench report：

| 判据 | 通过线 |
|---|---|
| `cmd_driver -SelfTest` | 34/34 |
| PTC steps | 4/4 执行、4/4 通过、0 失败 |
| PTC verification | 13/13 |
| PTC retries / error_classes | 0 / 空 |
| work-bench reference | 81/81 |
| work-bench negative | 20/20 |
| repeat | `repeat_all_ok=true`，`metrics.failed=0` |

任一条不满足，`agent_quality_gate.py` 返回 1，`release.yml` 的 `build` 不会启动。
门自身的输出是 `trit-agent-quality-gate/v1` JSON；`developer_only: true` 明确标记它
不是用户交付物。

## 4. 发布流程

### 4.1 发布前（本地，规范 worktree）

```bash
python scripts/release_preflight.py \
  --out target/release-manifest.json \
  --artifact target/release/trit-build
python scripts/release.py check
```

`release_preflight.py` 默认要求：git 顶层、origin `bfxh/trit`、分支 `main` 或
`release/*`、工作区 clean、canonical worktree。`--allow-noncanonical` 只给 CI/发布
专用 worktree，且会写进清单；本地发布不要用它绕过 clean 检查。

`release.py check` 以根 `Cargo.toml` 的 `[workspace.package] version` 为唯一版本来源，
校验最新 tag 与 CHANGELOG：当前版本必须有对应段落，或 `[Unreleased]` 非空。

### 4.2 打 tag

```bash
python scripts/release.py tag --apply --push
```

不带 `--apply` / `--push` 时是 dry-run。tag 形如 `vX.Y.Z`，必须与 workspace version
一致；`release.yml` 的 `guard` 会再次用 `cargo metadata` 校验 `trit-cli` 版本。

### 4.3 GitHub Release

tag push 后：

1. `guard`：先跑 `release_preflight.py --self-test` 与 `release.py --self-test`，再用
   `cargo metadata` 校验 tag 与 `trit-cli` 版本一致；
2. `agent-quality`：质量门 34/34、81/81、20/20、4/4、13/13；
3. `build`：Linux x86_64、Windows x86_64、macOS aarch64 构建、host test、`--version`
   smoke、打包；
4. `release`：只下载 `trit-*`，生成 `SHA256SUMS`，上传 Release assets。

手动 `workflow_dispatch` 可以跑质量门和构建，但 `release` job 有
`if: startsWith(github.ref, 'refs/tags/v')`，不会在分支上误发布。

## 5. 用户侧能看到什么

- Release 页面的三平台归档与 `SHA256SUMS`；
- GitHub 生成的 release notes；其中不应包含内部 job / receipt / 命令行细节；
- 用户文档中的安装、校验和与版本说明。

用户侧**看不到**：

- `agent-quality-gate.json`、`ptc-receipt.json`、`work-bench-report.json`；
- CI job 名、错误分类、self-test 明细、fixture 路径；
- 本地 `target/`、`artifacts/` 或 runner 临时目录。

发布说明只写用户能行动的信息：

```markdown
## trit vX.Y.Z

### 用户可见变更
- ……

### 升级与兼容性
- ……

### 校验
- 下载 `SHA256SUMS`，校验三平台归档。
```

不要写“哪个 CI job 红了又绿”“用了什么 receipt 路径”“错误分类怎么分”；
这些属于维护者排障信息，留在 CI 运行页或 engineering 文档。

## 6. CI 预算与并发

`scripts/ci_budget.py --offline` 是静态门：

- 每个 workflow 必须有 workflow 级 `concurrency`；
- 每个含 `runs-on` 的 job 必须有 `timeout-minutes`；
- 默认 job 预算 30 分钟；超过必须在 `.github/ci-budget.txt` 登记理由；
- `concurrency.cancel-in-progress: false` 只警告。

新增的 `agent-quality.yml` 与 `release.yml#agent-quality` 均为 10 分钟，不需要预算
豁免。`release.yml` 的 workflow 级 `concurrency` 保持 `cancel-in-progress: false`，
避免发布途中被新 tag 取消。

## 7. 排障

| 现象 | 先看 |
|---|---|
| `agent-quality` 红 | gate JSON 的 `reasons`、`artifacts`；再看 `ptc-receipt.json` 的 `steps[].status` 与 `error_class` |
| work-bench 负例未抓住 | `work-bench-report.json` 的 `negative.failures`，不要改参考产物来“修绿” |
| XLSX 任务失败 | preflight 是否报 `openpyxl` 缺失；CI 固定 `openpyxl==3.1.5` |
| `release` 没有发布 | 是否 tag 触发；`guard` 是否版本不一致；`build` 是否被 `agent-quality` 拦住 |
| Release 出现内部文件 | 检查 `download-artifact` 的 `pattern` 是否仍为 `trit-*`；不得改成 `*` |
| `mutation` PREFLIGHT 报“锚点出现 0 次” | 函数被拆分/移动；用 `rg -n '<old 锚点片段>' scripts/mutants/*.json` 找到条目，把 `file` 同步到新文件，再 `python scripts/mutation_check.py --pkg <crate> --only '<名字>'` 复跑该条 |
| PR 全绿但 `mergeStateStatus=BLOCKED` | 对照 `gh api repos/<owner>/<repo>/branches/main/protection/required_status_checks`；改 MSRV 或 job 名后必须同步分支保护的 `required_status_checks` |

## 8. 未落地 / 边界

- 分支保护与 required checks 是 GitHub 仓库设置，不在本仓库文件里；本文只保证
  workflow 名称与触发条件可供配置。改 `MSRV (1.xx)` 这类 job 名时，必须同时更新
  GitHub 分支保护的 required check 上下文，否则检查全绿但 PR 仍会 `BLOCKED`。
- 当前质量门矩阵是 Ubuntu + Windows；macOS 由 `release.yml#build` 的构建、host test
  与 smoke 覆盖，未单独跑 Python 质量门。
- 没有制品签名、SLSA provenance 或可重复构建声明；`SHA256SUMS` 只做传输完整性校验。
- `ci_budget.py --offline` 只做静态预算检查，不代替真实 runner 的耗时观测。
