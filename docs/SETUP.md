# 安装与上手（SETUP）

> 目标：新机器上的新同事（或半年后的自己）照着这份文档，**不踩我们踩过的坑**就能把仓库跑起来。
> 三进制主库 `bfxh/trit` 是一个 45 crate 的 Cargo workspace；下面所有命令都在仓库根目录执行。

## 1. 环境要求

| 项 | 版本 / 说明 |
|---|---|
| Rust | **1.88+**（`edition = 2024`，`Cargo.toml` 的 `rust-version = "1.88"`） |
| 安装方式 | [rustup](https://rustup.rs)：`rustup toolchain install stable && rustup default stable` |
| 组件 | `rustup component add rustfmt clippy` |
| OS | Windows 10/11（主开发环境）、Linux、macOS 均可；Windows 上建议用 Git Bash 或 PowerShell 7 |
| Python | 3.11+（仅校验脚本需要：`scripts/module_budget.py`、`scripts/mutation_check.py` 等） |
| 磁盘 | 见 §5。**至少留 20 GB**，全量 debug+release 构建的 `target` 就是 10–20 GB 量级 |

## 2. 一次性配置

```bash
rustup component add rustfmt clippy
cargo install cargo-sweep        # 可选：清理旧编译产物
cargo install cargo-mutants      # 可选：跑变异检查
git config user.name  "你的名字"
git config user.email "你的邮箱"   # 不要用任何 AI 厂商账号，见 AGENTS.md
```

磁盘紧张时把编译产物挪到别的盘（本机就是这么配的，仓库里的 `.cargo/config.toml` 是**本机策略**，别人克隆后按自己盘符改或删掉）：

```toml
# .cargo/config.toml
[build]
target-dir = "D:/cargo-target/trit"
```

## 3. 克隆与目录规范（最重要的一条）

**一个主工作树，分支验证用 `git worktree`，不要重复 `git clone`。**

```bash
git clone git@github.com:bfxh/trit.git ~/wt/trit
cd ~/wt/trit
git worktree add ../trit-pr373 pr-373   # 验证某个 PR 分支：共享 .git，不重复下载
git worktree remove ../trit-pr373       # 用完就删
```

为什么：一份 clone 本身只有 ~18 MB，但每份 clone 只要本地编译一次，就会各自长出 1–20 GB 的 `target`。历史上这台机器上出现过 **7 份同一个仓库的 clone**（`trit-cram-work` 551 MB、`trit-e3-doc*` 三份各 18 MB、`trit-wt-pr486`、`.cache/trit-pr373`、`wt/tconv-cap`），外加一个 **2.1 GB 全是 `target`** 的无 git 目录 `driftnet-e3`——这些才是"每个项目十几 G"的真正来源。

命名约定：

- 主工作树：`wt/trit`（或现有 `wt/tconv-cap`，二选一，别并存）
- 分支验证：`wt/trit-wt-<pr号>`，用完删
- **不要**用 `xxx-docs-eval-baselines` 这种从分支名派生的目录名——半年后没人知道它对应哪个分支
- `.cache/` 只放可随时删除的缓存，不放项目

## 4. 构建与测试

```bash
cargo build                     # 全量构建
cargo test --workspace          # 全部测试（45 crate，首次较慢）
cargo test -p trit-export       # 只测某个 crate
```

**三道门**（提交前至少跑前两条）：

```bash
cargo fmt --all -- --check                       # 1. 格式
python scripts/module_budget.py                  # 2. 模块行数台账（见 §6）
python scripts/mutation_check.py                 # 3. 变异检查（可选，慢）
```

`.agent/checks/registry.json` 里登记了 90 条「检查」（`kind = cmd` 的会跑对应命令），CI 和本地都可以按 id 复跑。

## 5. 磁盘与 `target`

- 单份 `target`（debug+release+测试）约 **10–20 GB**；共享 `CARGO_TARGET_DIR` 时所有分支共用同一份（省盘，但切分支会触发重编）。
- 清理：`cargo clean`（全清）、`cargo sweep -s`（按时间清旧产物）。
- 本机现状：`D:/cargo-target` 12 GB、`D:/cargo-target-chunk` 1.4 GB，C 盘与 E 盘都已接近满——**新克隆前先看盘**。

## 6. 模块行数台账（`module_budget`）

仓库对每个 `.rs` 有行数上限，规则是**只收紧、不抬高**：

- 新增文件：`python scripts/module_budget.py --update` 登记
- 既有文件超限：**拆文件**（把函数/模块搬到子模块），不要改台账上限
- 常见卡点：`rustfmt` 会把 `let ... else` 和多行 `if` 强制拆开，所以"加一行检查"经常变成 +3 行；此时正确做法是**拆分**而不是压缩

## 7. 仓库卫生与"bug 太多"的机制

这个仓库不靠记忆管 bug，靠三样东西：

| 机制 | 位置 | 用途 |
|---|---|---|
| 错误台账 | `docs/ERRORS.md`（L 编号） | 每条真实踩过的坑：现象 / 根因 / 修法 / 固化为哪条检查 |
| 检查登记 | `.agent/checks/registry.json`（C 编号） | 把台账条目变成可复跑的 `cmd` 或 `require` 检查 |
| 变异目标 | `scripts/mutants/*.json` | 证明测试真的能抓住回归（改了实现测试该红） |

新修一个 bug 的固定动作：先写「会失败的测试」→ 修 → 在 `ERRORS.md` 记一条 L → 在 registry 登记 C → 在对应 `mutants/*.json` 加一条变异。

## 8. 常见问题

**Q: `cargo test` 报 `target/debug...` 磁盘满？**
A: 换 `CARGO_TARGET_DIR` 到空间大的盘（§2），或 `cargo clean`。

**Q: `module_budget` 说 `GREW ... 要腾地方就拆，别抬台账`？**
A: 这是**规则**不是 bug：把出问题的函数搬到同目录子模块（`foo.rs` → `foo/bar.rs`），再 `--update` 登记新文件。

**Q: `rustfmt` 把我压成一行的代码拆回去了？**
A: 它有自己的宽度/结构规则（`let ... else`、`if` 分支），别跟它对抗——要么拆文件，要么接受多行。

**Q: 同一仓库要同时验证两个 PR，怎么办？**
A: `git worktree add`（§3），不要 clone。

**Q: 克隆后 `.cargo/config.toml` 指向的盘不存在？**
A: 该文件是本机策略，删掉或改成自己的盘符，不影响仓库其他部分。

## 9. 本机整理记录（2026-11-21）

一次全盘整理，**只动了本机目录，仓库内容不受影响**：

- 删除 6 份同一仓库的 clone（`trit-cram-work`、`trit-e3-doc`、`trit-e3-doc-docs-eval-baselines`、`trit-e3-doc-docs-hw-energy-paper`、`trit-wt-pr486`、`.cache/trit-pr373`）与 2 个无 git 的导出目录（`driftnet-e3` 2.1 G 纯 target、`.cache/trdiff`），共释放 **3.35 GB**
- **删除前逐份打包**：`git bundle create --all`（含全部本地分支、2 个 stash）+ `*-dirty.patch` + `*-status.txt`，归档在 `D:/trit-archive/2026-11-21/`（约 50 MB）
- 恢复某份：`git clone D:/trit-archive/2026-11-21/<name>.bundle <dir>`；未提交改动用同目录 `*-dirty.patch` 打回
- `D:/cargo-target` 删掉 `dn/`（对应已删的 `driftnet-e3`，1.5 GB）；`tconv-cap/` 的 11.85 GB **全部是 3 天内的活跃产物**，没有冷文件可扫，`cargo-sweep` 按时间清在此无事可做
- **`E:/cargo-target` 的 38.5 GB 全是死缓存**：7 个子目录（`trit` 18.2 G、`trit-wt-dimsweep-ci` 11.2 G、`driftnet` 6.5 G 等）对应的源码/worktree 都已不存在，整目录清空
- 现在只剩一个主工作树 `wt/tconv-cap`；`trit-agent`、`Projects/trit-tml`、`trit-fabric`、`trit-mc`、`wt/qingjian` 是**独立仓库**，`trit-equiv-work`（1 MB，无 git）保留
- **本次合计释放约 43.4 GB**（C 盘 3.35 G + D 盘 1.5 G + E 盘 38.5 G），E 盘可用从 4.7 G 回到 36 G
- 结论：这台机器上"每个项目十几 G"的真相是 **`target` 编译产物**，不是源码（每份 clone 只有 17–18 MB）；**重复 clone × 各自的 target × 每个 worktree 一个 target 子目录**才是放大器。`E:/sccache`（9.2 GB）与 `E:/trit-data`（41.7 GB）是编译加速缓存与数据集，未动
