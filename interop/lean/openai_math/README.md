# openai/math 评测接入（外置，固定提交）

本目录把 [`openai/math`](https://github.com/openai/math) 作为**外部评测语料**接入，不把它变成依赖：

- 不进入 Cargo/Lake 工作区，不引入 Mathlib；`interop/lean` 仍是无 Mathlib 的 4.35.0 项目。
- openai/math 的 Lean 库固定为 **Lean 4.34.1 + Mathlib + 30 个钉 SHA 的依赖**；两套 Lake 项目不混编。
- 评测分两层，证据等级分开写：
  1. **目录审计（A，已实测）**：只读固定提交的 `lean/formalization.yaml`、`lakefile.lean` 和 comparator 配置，检查文件/模块/定理名/依赖钉是否自洽。
  2. **Comparator 动态校验（未实测，只有脚本）**：需要 Linux + `comparator` + `landrun` + `lean4export`，构建 Mathlib 依赖后才能跑；不写入 CI，不声称已通过。

## 固定快照

| 项 | 值 |
|---|---|
| 仓库 | `https://github.com/openai/math` |
| 提交 | `4eaee87548250b2682affda7d05d947c05554bae` |
| 目录版本 | `formalization.yaml` v0.4 |
| Lean | `leanprover/lean4:v4.34.1` |
| 依赖 | 30 个 git requirement 全部钉 40 位 SHA；Mathlib `d13f23b` |
| 观测日期 | 2026-11-08 |

## 审计基线（A）

运行 [`audit.py`](audit.py) 得到的基线见 [`results/2026-11-08-4eaee87.json`](results/2026-11-08-4eaee87.json)：

- `sources` **176**、`main_results` **208**、唯一 comparator 配置 **192**（有配置承载多个定理，最多 6 个）。
- comparator 目录：**419** 个 `.json`、**678** 个 `.lean`；其中 **227** 个 `.json` 不被 `status.main_results` 引用。
- 208 条 main result 全部满足：catalogue 文件存在、comparator 配置存在且可解析、`declaration` 在 `theorem_names` 中、`solution_module`/`challenge_module` 对应 `.lean` 文件存在。
- `permitted_axioms` 全部为 `{propext, Quot.sound, Classical.choice}`；`enable_nanoda`：false 199、true 3、缺省 6（按 comparator 配置计）。
- 审计**不**证明这些定理被 Lean kernel 接受，也不证明 comparator 通过；动态校验未运行。

### 口径警告

- openai/math README 的“300 / 719 top-line results ~42%”是**二手口径（B）**；`formalization.yaml` 的 `status.main_results` 只有 208 条，二者不能直接互相替代，审计不强行对齐。
- comparator 目录的 419 个配置不是 419 个主结果：只有 192 个被当前 catalogue 引用；其余可能是支持结果、旧配置或尚未挂到 catalogue 的条目。
- 本仓 `interop/lean` 的 738 条 ternary conformance 与 openai/math 的数学形式化是两套独立评测，不合并计数。

## 运行

```sh
# 1) 稀疏拉取固定提交（不含 Mathlib/OAI blob 主体）
interop/lean/openai_math/fetch_sparse.sh /tmp/openai-math

# 2) 结构审计（stdlib only；失败返回非 0）
python3 interop/lean/openai_math/audit.py \
  --repo /tmp/openai-math \
  --json /tmp/openai-math-audit.json
```

手动 Comparator（Linux，需要完整 checkout，未在 CI 运行）：

```sh
interop/lean/openai_math/run_comparator.sh \
  --repo /path/to/openai-math \
  --out /tmp/comparator.json \
  ComparatorChallenges/QuasiRiemannHypothesis.json
```

- 前置：`comparator`、`landrun`、`lean4export` 在 `PATH`；按 openai/math 的 comparator README 安装。
- 脚本会校验提交与 toolchain，然后执行 `lake update`、`lake exe cache get`、`lake env comparator <challenge>`。
- 这是**可信语料**的手动校验，不额外用 `systemd-run` 隔离；不要把不可信 solution 交给它。
- openai/math 固定 4.34.1；虽然 Lean 4.35 已把 comparator 并入核心（`lake comparator`），本接入不强制升级语料。

## 自测与门禁

`audit.py` 自带离线自测（不需要网络、不需要 Lean）：

```sh
python3 interop/lean/openai_math/audit.py --self-test
# self-test: 32/32 passed
```

- 正例 + 逐项反证：提交/toolchain/catalogue 版本、脏工作树、缺失 OAI 文件、缺失/不可读 comparator
  配置、`declaration` 不在 `theorem_names`、solution/challenge 模块缺失、未钉 SHA 的 require、
  不完整条目、非标准公理、未引用配置、nanoda 分布、共享配置、非 git 目录、红报告渲染。
- 变异登记 `scripts/mutants/interop__lean__openai_math__audit.py.json` 25 条，逐条用自测判红：
  `python3 scripts/mutation_check.py --pkg interop/lean/openai_math/audit.py`
  ⇒ **caught 25 / 存活 0 / 假杀 0**（本地实测）。
- JSON 报告固定 LF 行尾；固定提交复跑的输出与 `results/2026-11-08-4eaee87.json` **逐字节一致**
  （`cmp` 通过）。CI 也会跑自测并对基线做 `diff`，漂移就判红。

## CI

`openai-math-audit` workflow 仅 `workflow_dispatch` 手动触发，执行稀疏拉取 + 审计；Comparator 动态校验不进 CI（成本与 landrun 依赖不适合默认跑道）。

## 证据边界

| 结论 | 等级 |
|---|---|
| 固定提交的 catalogue 计数与结构自洽 | A（本仓脚本实测） |
| README 的 300/719 覆盖率 | B（上游陈述，未复算） |
| Comparator/kernel 实际通过 | 未运行（脚本已备，不能算结果） |
| 审计脚本自身的正例/反例与变异钉住 | A（32 条断言；25 条变异 caught 25 / 存活 0，本地实测） |
