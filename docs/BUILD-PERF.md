# 构建性能：实测、取舍与落地（2026-11-10）

> 口径：所有读数都是本机或 CI 上**实跑**出来的，命令随读数给出；论文/博客数字标来源并注明"未复现"。
> 本文回答一个问题：**编译时间还能压到多低，压在哪一层才有用。**

## 0. 一句话结论

本机的边际编译已经是"根代码级"（改一个 crate → `cargo check` **3.1s**、测试二进制 **5.4s**；
从零 `cargo check --workspace` **10.9s**，24 核 / NVMe）。真正的浪费不在本机，而在 CI：
**只改脚本的 PR 也在全量重编 Rust 代码**（Windows job 12 分钟里 10 分钟花在没变的 Rust 上）。
本次落地的是把这类改动从 cargo 矩阵里摘出去；本机加了 sccache，但它的收益有明确边界（见 §3）；
测试执行换 cargo-nextest（跨二进制并行，见 §8）。CI 上真正生效的编译缓存只有 `rust-cache` 的
target 目录缓存：workflow 装了 sccache 二进制，却从没设过 `RUSTC_WRAPPER` / `SCCACHE_GHA_ENABLED`，
它一次都没接管 rustc 调用——证据与更正见 §8。

## 1. 本机读数（24 核 Intel Ultra 7 270K Plus，NVMe SSD，rustc 1.99.0）

| 场景 | 命令 | 读数 |
|---|---|---|
| 从零全工作区检查（201 个编译单元） | `CARGO_INCREMENTAL=0 cargo check --workspace --locked`（空 target） | **10.9s** 墙钟 / 108.5s CPU 合计 |
| 改一个 crate 后增量检查 | 改动 `trit-core` 后 `cargo check -p trit-core` | **3.1s** |
| 改一个 crate 后测试二进制 | 同上后 `cargo test -p trit-core --lib --no-run` | **5.4s** |
| 从零全工作区构建（含 codegen+链接） | `CARGO_INCREMENTAL=0 cargo build --workspace --locked`（空 target） | **29.9s** |
| 同路径清空重建 + sccache 热缓存 | 同上（同 target 路径，先 `cargo clean`） | **21.3s（113 命中，-29%）** |
| 不同 target 目录 + sccache | 同上（换 target 目录） | **30s，0 命中** |

工作区规模：**37 个 crate、292 个锁定依赖包**。`cargo check --workspace` 的 201 个单元
在 24 核上被压进 11 秒，说明前端（parse/typecheck/borrow）已不是瓶颈；
再往下压的边际收益在**链接与重复编译**，不在"让 rustc 更快"。

## 2. 编译时间花在哪：分层看

1. **不重复编译**（缓存）：同一份代码第二次编 = 0。CI 已用 `Swatinem/rust-cache`；本机用 sccache（§3）。
2. **少编译**（作用域）：只编改动 crate 的反向依赖闭包（`scripts/ci_scope.py` 已做）。
   **本次补上的缺口**：改动集完全不含 Rust 面时，连闭包都不用算——直接跳过 cargo 矩阵（§4）。
3. **编得快**（并行/链接）：24 核 + dev 默认 `codegen-units=256` 已把前端并行拉满；
   Linux 自 Rust 1.90 起默认 `rust-lld`，链接这一层无需额外配置。
4. **测得快**（执行）：cargo-nextest 跨二进制并行（见 §8）。

## 3. sccache：能力边界比开关重要

本机已装 sccache 0.19.0，用户级配置：`rustc-wrapper = "sccache"`、缓存目录指向 SSD、
`SCCACHE_BASEDIRS` 归一化源码根。实测边界：

- **同一 target 路径**：清空后重建 29.9s → **21.3s**（113 命中）。对"删 target / 换 feature 重编"有效。
- **不同 target 路径（不同 worktree）**：**0 命中**。`--out-dir` 进入缓存键，
  `SCCACHE_BASEDIRS` 只归一化源码路径，救不了 target 目录名不同 ⇒ **25 个 worktree 互不共享**。
  共享 target 目录可以让命中成立，但会带来 cargo 锁竞争与指纹互相失效，不推荐。
- 结论：**sccache 的收益集中在"路径稳定"的场景**（本机同 worktree 重建、删 target 重编）；
  多 worktree 的快速切换靠的是增量编译（已默认开启）+ 24 核并行，而不是缓存。
  CI 表面上"每次同一路径"，可这个前提从未兑现：workflow 只装了 sccache 二进制、没接到 rustc 上，
  收益一次也没有发生（见 §8「归因更正」）。

停用方法：删掉用户级 cargo 配置里 `[build] rustc-wrapper` 两行，或用
`RUSTC_WRAPPER=` 单次覆盖；没有其它副作用。

## 4. CI 实测拆解与本次改动

以 PR #238（只改脚本 + 变异登记表）的 Windows job 为例，12 分钟的总时长拆到 step：

| step | 耗时 | 说明 |
|---|---|---|
| Test | 240s | 全量 `cargo test --workspace` |
| Test (trit-gpu gpu feature, shader smoke) | **372s** | 编 wgpu/`gpu` feature + 1×1×1 冒烟 |
| Clippy | 41s | 全量 `--all-targets` |
| rust-cache 恢复 | 19s | 缓存命中后仍需恢复 |
| fmt / 其它 | < 10s | |

同一提交 ubuntu 3m40s、macOS 4m15s——**Windows 的 12 分钟里约 10 分钟是没必要的**：
PR 没碰任何 Rust 文件。落地改动（`.github/workflows/ci.yml`）：

- "Detect docs-only" 步骤扩展为 **"改动集不含 Rust 面"**（输出键名沿用 `only_docs`）：
  `docs/`、`*.md`、`scripts/`、`.github/`、`sweeps/`、`tests/`（仅夹具）、`other-languages/`、
  `benchmarks/`、`t3/` 全部命中时，跳过 fmt/clippy/test 及其 feature 变体。
- 安全性依据：主 workspace 只含 `crates/*`；`other-languages/`、`benchmarks/` 不是成员；
  `t3/` 是独立 workspace（`t3.yml` 自跑）；Rust 测试不读取 `scripts/` 下的任何文件
  （`trit-orch` 的对拍测试读的是仓内 JSON 夹具）。
- 门族 job（module budget / mutation / test surface / hazard / secret 等）不在这个 job 里，照常全跑。
- 预期：脚本类 PR 的 Windows job 12min → **约 1 分钟**（只剩检出、装 Rust、缓存与 fmt 判定以外的门）。

## 5. 未采纳及原因（都试过或查过，不是没想）

| 技术 | 结论 | 原因 |
|---|---|---|
| 精简 dev 调试信息（本仓 `line-tables-only`，依赖 `false`） | **已落地**（§10） | 11-11 只测了 GPU 冒烟那一步：`CARGO_PROFILE_DEV_DEBUG=0` **27.2s vs 27.5s**，那一步瓶颈是 wgpu 编译；11-15 按整个测试构建重测：测试二进制 75% 的字节是调试信息，体积减半、构建快约 35% |
| nightly `-Zthreads` / `-Zshare-generics` | 观望 | 外部实测约 22.7%（[Depot](https://depot.dev/blog/guide-to-faster-rust-builds-in-ci)，未复现）；要求 nightly，与本仓 stable/MSRV 口径冲突 |
| Cranelift 后端（dev 构建） | 未采纳 | 仍需 nightly 组件；外部报告在低层依赖上直接失败（同上来源） |
| mold（Linux 链接器） | 不需要 | Linux 自 Rust 1.90 起默认 `rust-lld`（[Rust 官方博客](https://blog.rust-lang.org/2025/09/01/rust-lld-on-1.90.0-stable/)），再换收益边际 |
| cargo-nextest（测试执行） | **已落地** | 跨二进制并行；本机热态 63.7s → 58.4s（2502 条测试，24 核），CI 见 §8 |
| CI 上的 sccache（GHA 后端） | **未生效**（只装了二进制） | workflow 没设 `RUSTC_WRAPPER` / `SCCACHE_GHA_ENABLED`，sccache 不接管 rustc；CI 日志 `Compile requests 0`。CI 的收益全在 `rust-cache` + nextest（§8） |
| 多 worktree 共享 target 目录 | 不推荐 | 锁竞争 + 指纹互相失效；实测跨目录 sccache 0 命中，共享目录也难两全 |

## 6. 复算命令

```bash
# 本机全工作区冷检查（换一个空 target 目录，去掉本机配置里的 target-dir 影响）
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=<空目录> cargo check --workspace --locked

# 边际循环
#  （改动任意 crate 后）
cargo check -p <crate>            # 期望 ~3s 级
cargo test -p <crate> --lib --no-run

# sccache 读数
sccache --show-stats | grep -E "Cache hits|misses"

# CI 各 step 耗时（把 <run-id> 换成任意一次 CI 运行）
gh api repos/bfxh/trit/actions/runs/<run-id>/jobs --jq '.jobs[] | {name, started_at, completed_at}'
```

## 7. 回收

- CI 作用域改动只影响 `if:` 条件，回滚 = 恢复那一行正则。
- 本机 sccache：删用户级 cargo 配置的 `[build]` 两行即可；缓存目录可直接删除（可重建）。

## 8. 测试执行：cargo-nextest（2026-11-11 落地）

- **本地（24 核，热态，2502 条测试 / 1 skipped）**：`cargo test --workspace --locked` **63.7s**，
  `cargo nextest run --workspace --locked` **58.4s**（其中执行 50.0s）——本机核心多，收益只有 ~8%；
  CI 的 4 核 runner 上跨二进制调度收益更大（见下）。
- **CI 首轮（缓存未命中，整仓冷编译）**：Windows job 727s → 700s、macOS 255s → 217s、Ubuntu 220s → 200s；
  其中 `Test` step 240s → 218s（另有独立 doctest step 7s，nextest 不跑 doctest，必须单列）。
- **CI 稳态（`rust-cache` 命中，同一提交重跑）**：Windows job **727s → 493s（-32%）**，其中 GPU 冒烟 372s → 219s、Test 240s → 167s；macOS 255s → 222s、Ubuntu 220s → 200s。
  说明：target 缓存要第二趟才热（首趟全编），这一趟的收益来自恢复 `rust-cache` + nextest 跨二进制并行。
- 成本：安装步骤 sccache 1s（装了但没接上 rustc，见下）、nextest 3s（`taiki-e/install-action`，版本钉死）。
- 口径：`cargo nextest run $SCOPE_ARGS --locked` 与 `cargo test $SCOPE_ARGS --locked --doc` 成对出现，
  少任何一条都算测试面缩水。

**归因更正（2026-11-15）**：上面首轮/稳态两处，连同本节原标题，原先写的是「sccache 冷缓存 /
sccache 命中」——错。CI 里的 sccache 从未生效，`rust-cache` 是唯一的编译缓存。证据：

- `git log -S "RUSTC_WRAPPER" -- .github/workflows/ci.yml` 与
  `git log -S "SCCACHE_GHA_ENABLED" -- .github/workflows/ci.yml` 两条都是空：这两个变量从没进过这个文件。
  `git log -S "sccache-action" -- .github/workflows/ci.yml` 只有一条：`05131a58`（#404）。
- #404 分支上的 ci.yml（`git show 0a8d2743:.github/workflows/ci.yml`）只有 `sccache-action` 一个安装
  步骤，没有任何一处把 sccache 设成 rustc 包装器；`sccache-action` 负责装二进制并导出 `SCCACHE_PATH`，
  不接管 cargo 调用。
- main 最近一次 CI（run 43591149930，Windows job 591s）的日志：`Compile requests 0`、`Cache hits 0`、
  `Cache location Local disk`（用 runner 本机盘，不是 GHA 后端）；同一 job 的 `rust-cache` 是
  `full match: true`。同一现象在 #502 的四个 job 上重复（§10）。
- 所以 727s → 493s 该记在「`rust-cache` 命中 + nextest」上，不是 sccache。这一对数字出自 #404 当时
  的运行，分支已随合并删除、CI run 只剩最后一次（那次改动集只有 `.github/`，被 `only_docs` 整段跳过、
  什么也没编），现在无法再从 CI 复算——本篇按「当时读数、机制可推、不再复算」处理，可复算的替代读数
  是 §10 第四轮。
- 要真接 sccache 是另一件事：workflow 需设 `RUSTC_WRAPPER` 与 `SCCACHE_GHA_ENABLED: true`，再接一轮
  实测。本 PR 不接，也不认领它过去的「收益」。


## 9. worktree 的产物落盘：E: 策略自动传播（2026-11-14 落地）

§3 的遗留缺口：`target-dir` 指向产物盘 `cargo-target/...` 的本机覆盖文件只在主 worktree 手工放过
（`.git/info/exclude` 不入库），`git worktree add` 出来的每个 worktree 都没有它——构建产物
写回 D 盘直到写满（2026-11-14 实际发生：D 盘 100%，链接报 `No space left on device`）。

本次落地两层：

- **本机自动层**：`.git/hooks/trit-hooks.py`（post-checkout，不入库的本机资产）在
  checkout/worktree add 时幂等生成 `.cargo/config.toml`；条件：策略根（产物盘的
  `cargo-target/` 目录）存在 + 顶层有 `Cargo.toml` + 配置尚不存在。生成失败静默，钩子永不阻塞 checkout。
- **入库层**：`scripts/wt_target.py`（`--all` / `--status` / `--self-test`）把同样的
  逻辑做成可复制、可审计的批量工具；`.gitignore` 收 `.cargo/config.toml`；
  `AGENTS.md` 工程口径加一行指引。新 clone 没有本机钩子时用它补齐。

仍然每 worktree 独立 target 目录，不共享（§3：共享 = cargo 锁竞争 + 指纹互相失效）；
`SCCACHE_BASEDIRS` 已含产物盘的策略根，跨目录重建有 sccache 兜底。

## 10. 构建瘦身：收窄补缺口、GPU 冒烟并行、调试信息减重（2026-11-15 落地）

**收窄补缺口（`scripts/ci_scope.py`）**：`scripts/**` 原先在「归不到 crate ⇒ 全量」名单里，
可它不参与 Rust 编译（§4 已据此把纯脚本 PR 整个跳过）。抽样 40 个已合并 PR：36 个走了全量，
其中 32 个只是因为顺手改了 `scripts/` 下的台账（基线 / 登记表）。改为 `NEUTRAL_DIRS` 略过后，
同一样本全量 36 → **4**，按 crate 收窄 30 个，整段跳过 Rust 矩阵 6 个；剩下 4 个
（`Cargo.lock`、`.gitignore`、workflow、闭包 38/45）都该全量。前提由代码守：`rust_reads`
扫每个成员的 `*.rs` 与 `Cargo.toml`，一旦出现对 `scripts/` 的引用（注释除外），这次运行就把它
踢出中性集、退回全量，并在日志里点名文件:行号。MSRV job 用同一把尺子收窄（之前每个 PR
都全量 build + test）。

**GPU 冒烟后台化（`.github/workflows/ci.yml`）**：1×1×1 冒烟在 Windows WARP 上是纯运行时间
（抽样 PR 263s，是该 job 516s 里最长的一段），和 nextest/doctest/clippy 互不依赖。改成两步：
Clippy 之后当场 `--no-run` 编好（编译错误当场判红），用例放后台跑；job 末尾「— wait」打印完整
日志、按退出码判红（后台进程失踪靠 20 分钟超时判红）。fmt/clippy 的快速失败不受影响。
引入它的 PR（#502）在 CI 上跑了三轮，三轮都是整仓冷编译（`rust-cache` 换代前恢复的是作废的
旧缓存，见下文「调试信息减重」）。冒烟在关键路径上只剩「— start」+「— wait」：97s / 50s / 137s
（原来串行 272–375s）。Windows job 528s / 436s / 689s，main 最近 6 次合并后全量 583–751s（常态
约 600s）；同一 job 三轮之间差 250s，冷编译下整 job 的快慢判不了。ubuntu / MSRV / macOS 同理
（274/214/306s、266/157/248s、350/341/329s，main 为 177–222s、164–251s、185–310s）。

**第四轮（暖缓存，run 43632123371，head `bd1dc0a4`，同一提交）**：前三轮恢复的都是换键前的旧键
（`v0-rust-check-Windows_NT-…`），full match 但产物作废，所以三轮都是整仓冷编译。第四轮恢复的是
带根 `Cargo.toml` 哈希的新键（`v0-rust-ed26d419…-check-Windows_NT-…`，`full match: true`），产物
有效；四个 job 的 `rust-cache` 全部 `full match: true`，sccache 仍 `Compile requests 0`：

| job | 第四轮（暖缓存） | main 暖缓存基线 |
|---|---|---|
| fmt + clippy + test (windows-latest) | **446s**（19:35:43→19:43:09） | 583–751s（常态约 600s） |
| fmt + clippy + test (ubuntu-latest) | **157s** | 177–222s |
| fmt + clippy + test (macos-latest) | **190s** | 185–310s |
| MSRV (1.88) | **171s** | 164–251s |

Windows 上冒烟在关键路径上只剩「— start」**14s**（19:37:09→19:37:23）和「— wait」**109s**
（19:41:12→19:43:01）。口径：同一提交、`rust-cache` full match、且是冷编译三轮之外的第四轮——只有
这一轮能和 main 的暖缓存基线比；前三轮的 job 耗时（528/436/689s）不能，原因见上。复算：`gh api
repos/bfxh/trit/actions/jobs/<job-id> --jq '.steps[] | {name, started_at, completed_at}'` 拿 step
级起止；是否 full match 看该 job 日志里 rust-cache 步骤的 `Restored from cache key "…" full match:
true`。

顺手修复：全量兜底分支漏设 `needs_build_gpu`（#376 引入），push main 时 trit-build 的
GPU 测试与 clippy 一直被跳过；现在补上（只会多跑）。

**调试信息减重（根 `Cargo.toml`）**：`[profile.dev] debug = "line-tables-only"`，
`[profile.dev.package."*"] debug = false`，test profile 继承 dev。panic 回溯仍有本仓文件:行号；
debug-assertions / overflow-checks / opt-level 不变——测什么、怎么判都没变，只是不再生成没人读的
完整调试信息。本机实测（x86_64-pc-windows-gnu，调试信息是嵌在二进制里的 DWARF，与 Linux 同形态；
`cargo test -p trit-wspace --no-run --locked`，空 target 目录，不经 sccache，调换顺序复测一轮）：

| 指标 | 改前 | 改后 |
|---|---|---|
| target 目录 | 4362 MB | 2184 MB（−50%） |
| 测试二进制（58 个）合计 | 2227 MB | 1094 MB（−51%） |
| 其中调试信息段 | 1675 MB（75%） | 579 MB（−65%） |
| 墙钟（两轮） | 40.3s / 38.2s | 25.9s / 25.5s（约 −35%） |

§5 那次 27.2s vs 27.5s 测的是 GPU 冒烟那一步（瓶颈在 wgpu 编译）；测试构建要链接几十个
测试二进制，调试信息的生成、搬运和写盘才是大头。CI 上链接同样变轻，`rust-cache` 要存取的
依赖产物也变小。要单步调试时本地临时把 `debug` 调回 `true`（不要提交）。

改 profile 必须让 CI 缓存换代：`rust-cache` 的键只哈希依赖（`Cargo.lock` 与各清单的依赖段），
不含 `[profile]`。#502 的前三轮 CI 都 full match 恢复了 main 的旧缓存（main 最近一次 push 也是
同一把键），产物在新 profile 下全部作废、整仓重编，随后又因「键已存在」不保存——合并后 main
也会一直这样。现在 ci.yml / mutation.yml 的 `rust-cache` 都带 `key: ${{ hashFiles('Cargo.toml') }}`，
根清单一改就换代（改依赖时 `Cargo.lock` 本来就会换键）。另：CI 里的 sccache 从来没生效过——
各 job 日志 `Compile requests 0`、缓存位置是本地盘（不是 GHA 后端）：`sccache-action` 只装二进制，
workflow 没设 `RUSTC_WRAPPER` / `SCCACHE_GHA_ENABLED`。核实结论（`git log -S` 逐条查过）：
`RUSTC_WRAPPER` 与 `SCCACHE_GHA_ENABLED` 在 `.github/workflows/ci.yml` 的历史里都是空，两个变量
从没进过这个文件；`sccache-action` 只在 #404（`05131a58`）引入过一次，且当时就没有配包装器。
所以 §8 里把收益记在 sccache 头上是错的，详见那一节的「归因更正」。接上它是另一件事，本 PR 没动。

**防复发（`scripts/worktree_guard.py` + `scripts/gitflow.py`）**：回收规则早在 AGENTS.md 里，
仍攒下 33 个已合 PR 的工作树——没人跑，也不删外置 target。现在 `--apply` 连带删除该树
`策略根/<树名>` 的外置 target（只删绝对路径、树外、同名、没有别的工作树指向的那份；策略根
本身名字对不上，永远不删）；当前工作树按 `git rev-parse --show-toplevel` 保护，主工作树显式保护；
`gitflow.py worktree add` 新建前自动跑一次（尽力而为，`TRIT_NO_AUTO_RECLAIM=1` 关闭）；自测接进
CI 的 recovery job。仓库设置已开「合并后自动删除远端分支」。

**回滚**：每一项都是独立的一处——`NEUTRAL_DIRS` 置空、ci.yml 两步并回一步、删掉 `Cargo.toml`
末尾两段 profile、设 `TRIT_NO_AUTO_RECLAIM=1`。
