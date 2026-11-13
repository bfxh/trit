# 构建性能：实测、取舍与落地（2026-11-10）

> 口径：所有读数都是本机或 CI 上**实跑**出来的，命令随读数给出；论文/博客数字标来源并注明"未复现"。
> 本文回答一个问题：**编译时间还能压到多低，压在哪一层才有用。**

## 0. 一句话结论

本机的边际编译已经是"根代码级"（改一个 crate → `cargo check` **3.1s**、测试二进制 **5.4s**；
从零 `cargo check --workspace` **10.9s**，24 核 / NVMe）。真正的浪费不在本机，而在 CI：
**只改脚本的 PR 也在全量重编 Rust 代码**（Windows job 12 分钟里 10 分钟花在没变的 Rust 上）。
本次落地的是把这类改动从 cargo 矩阵里摘出去；本机加了 sccache，但它的收益有明确边界（见 §3）；
测试执行换 cargo-nextest（跨二进制并行），CI 每轮编译再叠一层 sccache GHA 缓存（见 §4）。

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
- 结论：**sccache 的收益集中在"路径稳定"的场景**（CI 每次同一路径、本机同 worktree 重建）；
  多 worktree 的快速切换靠的是增量编译（已默认开启）+ 24 核并行，而不是缓存。

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
| `CARGO_PROFILE_DEV_DEBUG=0` | 未采纳 | 本机对照实测 **27.2s vs 27.5s**，无差异——该步骤瓶颈是 wgpu 编译，不是 PDB |
| nightly `-Zthreads` / `-Zshare-generics` | 观望 | 外部实测约 22.7%（[Depot](https://depot.dev/blog/guide-to-faster-rust-builds-in-ci)，未复现）；要求 nightly，与本仓 stable/MSRV 口径冲突 |
| Cranelift 后端（dev 构建） | 未采纳 | 仍需 nightly 组件；外部报告在低层依赖上直接失败（同上来源） |
| mold（Linux 链接器） | 不需要 | Linux 自 Rust 1.90 起默认 `rust-lld`（[Rust 官方博客](https://blog.rust-lang.org/2025/09/01/rust-lld-on-1.90.0-stable/)），再换收益边际 |
| cargo-nextest（测试执行） | **已落地** | 跨二进制并行；本机热态 63.7s → 58.4s（2502 条测试，24 核），CI 见 §8 |
| CI 上的 sccache（GHA 后端） | **已落地** | 路径固定 ⇒ 可命中；与 `rust-cache` 互补（target 缓存淘汰/换 feature 组合时兜底），CI 见 §8 |
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

## 8. 测试执行：cargo-nextest + sccache（2026-11-11 落地）

- **本地（24 核，热态，2502 条测试 / 1 skipped）**：`cargo test --workspace --locked` **63.7s**，
  `cargo nextest run --workspace --locked` **58.4s**（其中执行 50.0s）——本机核心多，收益只有 ~8%；
  CI 的 4 核 runner 上跨二进制调度收益更大（见下）。
- **CI 首轮（sccache 冷缓存）**：Windows job 727s → 700s、macOS 255s → 217s、Ubuntu 220s → 200s；
  其中 `Test` step 240s → 218s（另有独立 doctest step 7s，nextest 不跑 doctest，必须单列）。
- **CI 稳态（sccache 命中，同一提交重跑）**：Windows job **727s → 493s（-32%）**，其中 GPU 冒烟 372s → 219s、Test 240s → 167s；macOS 255s → 222s、Ubuntu 220s → 200s。
  说明：sccache 的增益在第二轮才体现（首轮全 miss），rust-cache 与它互补。
- 成本：sccache 安装 2s、nextest 安装 3s（`taiki-e/install-action`，版本钉死）。
- 口径：`cargo nextest run $SCOPE_ARGS --locked` 与 `cargo test $SCOPE_ARGS --locked --doc` 成对出现，
  少任何一条都算测试面缩水。


## 9. worktree 的产物落盘：E: 策略自动传播（2026-11-14 落地）

§3 的遗留缺口：`target-dir = E:/cargo-target/...` 的本机覆盖文件只在主 worktree 手工放过
（`.git/info/exclude` 不入库），`git worktree add` 出来的每个 worktree 都没有它——构建产物
写回 D 盘直到写满（2026-11-14 实际发生：D 盘 100%，链接报 `No space left on device`）。

本次落地两层：

- **本机自动层**：`.git/hooks/trit-hooks.py`（post-checkout，不入库的本机资产）在
  checkout/worktree add 时幂等生成 `.cargo/config.toml`；条件：策略根 `E:/cargo-target`
  存在 + 顶层有 `Cargo.toml` + 配置尚不存在。生成失败静默，钩子永不阻塞 checkout。
- **入库层**：`scripts/wt_target.py`（`--all` / `--status` / `--self-test`）把同样的
  逻辑做成可复制、可审计的批量工具；`.gitignore` 收 `.cargo/config.toml`；
  `AGENTS.md` 工程口径加一行指引。新 clone 没有本机钩子时用它补齐。

仍然每 worktree 独立 target 目录，不共享（§3：共享 = cargo 锁竞争 + 指纹互相失效）；
`SCCACHE_BASEDIRS` 已含 `E:/cargo-target`，跨目录重建有 sccache 兜底。
