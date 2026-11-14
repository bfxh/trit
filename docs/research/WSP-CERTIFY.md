# `certify`：保持集副作用证书（P0-2）

## 问题

连续权重空间里的模型编辑（ROME / MEMIT / AlphaEdit）没有可复现的判等：编辑后只能比较
logits 的相对漂移，"保持集（preservation set）上有没有改坏"永远用容差说话，报告无法
被第三方逐条复核。

三值权重把编辑变成**逐位替换**，于是同一件事可以写成证书：编辑前后各跑一遍保持集，
逐样本比较三值输出，用 `==` 而不是误差阈值。

## 判定

一次编辑要同时满足两件事才算成立：

- **保持集**：与本次编辑无关的样本（输入 + 期望输出）必须逐样本仍然正确；
- **编辑集**（可选）：本次编辑必须达成的样本必须全部正确。

逐样本五分类，只有一类算违规：

| 判定 | 编辑前 | 编辑后 | 违规 |
|---|---|---|---|
| `preserved` | 对 | 对 | 否 |
| `fixed` | 错 | 对 | 否（保持集里本不该发生，单独计数） |
| `regressed` | 对 | 错 | **是** |
| `drifted` | 错 | 错，输出变了 | 否 |
| `stale` | 错 | 错，输出逐位相同 | 否 |

证书同时记补丁自身的事实：`noop`（目标值已等于当前值）、`repeated`（同一位置被写多次）、
`touched`。它们不改变行为，但会让"编辑了几位"的读数虚高。

## 用法

```bash
# 补丁 + 保持集（带标签的 oracle）
trit-wspace certify base.wsp --patch patch.json --keep keep.json \
  --edit edit.json --certificate certificate.json --out edited.wsp --strict

# 直接比两个工件（外部量化器产出的 edited.wsp）
trit-wspace certify base.wsp --edited edited.wsp --keep keep.json

# 从网络自身行为采样保持集：证明"行为一字未变"，不需要外部标签
trit-wspace certify base.wsp --patch patch.json --keep-samples 256 --seed 7
```

- `--keep-samples N [--seed S]`：随机三值输入 + 当前输出作为期望（`certify::behavior_set`），
  宽输入也能用（`Oracle::sampled_behavior` 受穷举域 2^20 上限约束，这里不受）；
- `--strict`：证书不成立时非零退出（CI 门禁用）；
- `--allow N`：允许 N 条 violation（默认 0）；
- `--certificate <path>`：证书 JSON 落盘，schema `trit-wspace.certificate.v1`。

## 真实读数

all-MiniLM-L6-v2 `encoder.layer.0.attention.self.query.weight`（384×384，147,840 trits）
导入为 WSP1；保持集用 `--keep-samples`（随机三值输入 + 当前输出），`--seed 7`。

| 场景 | 样本 | preserved | regressed | certified | patch 事实 |
|---|---:|---:|---:|---|---|
| `delta-patch.json`（2026 trits，合成 Δ + trim 0.2） | 64 | 0 | 64 | false | edits 2026，touched 2026 |
| `delta-edited.wsp`（同一编辑，直接比工件） | 64 | 0 | 64 | false | — |
| `delta-edited.wsp`（换 seed=11） | 256 | 0 | 256 | false | — |
| 空操作单点（`to` 等于现值） | 64 | 64 | 0 | true | noop 1 |
| 全 384 行 `set_scale 2.0` | 64 | 64 | 0 | true | scales 384，touched 0 |
| 真正改一格（row 5 col 7：`0`→`+`） | 64 | 62 | 2 | false | touched 1 |

读法：

1. 合成 Δ 只改 2,026 个 trit（1.4%），却把 64/64 样本的三值输出全改了——在这种权重上，
   "少量编辑"与"行为不变"没有关系，任何不做保持集验证的编辑报告都不可信；
2. 单点改动只命中 2/64：随机保持集对**小**编辑不敏感。要抓小编辑，必须有真实任务的
   保持集，否则证书会给出"通过"但覆盖不足——这是本方案的明确边界；
3. `set_scale` 改 384 行、`touched=0`、0 violation：证书能区分"改 f32 读数"与"改行为"，
   这正是三值规范自由度（正 scale 不改行为）的直接体现。

## 未证栏

- MiniLM `query` 层没有配套任务，上述保持集是**行为快照**，不是真实 NLP 保持集；
  分布外输入不在证书范围内；
- 没有覆盖 384 维输出的真实任务 oracle，"编辑集是否达成"这条门禁在真实权重上还没跑过；
- 证书只回答"在这份保持集上没改坏"，不回答"编辑是否学到了目标行为"——那需要编辑集；
- `noop`/`repeated` 只覆盖 `set_trit`/`set_bias`/`set_scale`/`swap_rows`；tile 与行级操作
  的等价性交给 `Patch::cost` 的 `touched` 读数。

## 接口

- `certify::certify_nets(base, edited, keep, edit, options)`：直接比两个工件；
- `certify::certify(base, patch, keep, edit, options)`：应用补丁再出证书；
- `certify::apply_and_certify(...) -> (Certificate, Net)`：CLI 落盘用；
- `certify::patch_facts(base, patch)`：空操作/重复目标统计；
- `certify::behavior_set(net, samples, seed)`：随机三值输入的行为快照保持集；
- `Verdict` / `SampleRow` / `KeepReport` / `PatchFacts` / `Certificate`（均 `to_json`）。
