# `task-merge --trim`：TIES 的裁剪步骤（P1）

## 缺口

TIES-Merging（arXiv 2306.01708）三步：**trim** → **elect sign** → **disjoint merge**。
本仓已有后两步（加权投票 + 符号合成），缺第一步：每个坐标上所有任务的票都进投票。
`MergeReport.dropped` 字段一直是 0，就是给这一步留的位置。

## 三值空间里的 trim

f32 里 trim 是“保留 |Δ| 最大的 top-k%”，幅度连续。三值权重里一票的幅度只有三档：

| |Δ| = |target − base| | 含义 |
|---|---|
| 2 | 符号翻转（`-`↔`+`） |
| 1 | 与 0 互换 |
| 0 | 目标等于 base（不产生 edit，不会进票） |

所以 trim 在三值里退化成**按档位裁剪**：先保符号翻转的票，再保 ±0 互换的票。

```rust
pub struct TrimOptions {
    pub top: Option<TrimTop>, // Count(n) 或 Fraction(ratio)
    pub margin: f64,          // |Σ wᵢ·Δᵢ| < margin 的坐标整条丢弃
}
```

- `TrimTop::Count(n)`：每坐标保留 |Δ| 最大的 n 票；|Δ| 相同按权重降序、再按任务顺序（稳定）。
- `TrimTop::Fraction(ratio)`：每坐标保留 `ceil(ratio × 票数)` 票，至少 1 票。
- `margin`：裁剪后票和的绝对值低于门槛就保留 base，并把该坐标计入 `dropped`。

`TrimOptions::default()` 与 `merge_tasks_weighted_report` 逐位一致（有测试守住）。

## 用法

```bash
# 每坐标只让 |Δ| 最大的一票说话（TIES k=1 的三值特例）
trit-wspace task-merge base.wsp a.wsp b.wsp c.wsp --trim 1

# 保留 40% 的票（每坐标向上取整，至少 1 票）
trit-wspace task-merge base.wsp a.wsp b.wsp c.wsp --trim 0.4

# 票差不足 1.5 的坐标整条丢弃
trit-wspace task-merge base.wsp a.wsp b.wsp c.wsp --trim-margin 1.5

# 两者可叠加，但见下面的警告
trit-wspace task-merge base.wsp a.wsp b.wsp c.wsp --trim 1 --trim-margin 1.5
```

`--trim` 与 `--dare-rate/--dare-per-task` 互斥：DARE 也有自己的 `dropped` 语义，两条路径同时开会把两种“丢弃”混在一个字段里。

## 真实读数

### MiniLM `query` 384×384（前 80 行 × 300 列，三任务各 24,000 条编辑）

| 场景 | voted | ties | changed | dropped |
|---|---:|---:|---:|---:|
| 无 trim | 24,000 | 2,543 | 21,457 | 0 |
| `--trim 1` | 24,000 | 0 | 24,000 | 0 |
| `--trim-margin 1.5` | 24,000 | 0 | 13,429 | 10,571 |
| `--trim 1 --trim-margin 1.5` | 24,000 | 0 | 0 | 24,000 |
| `--trim 0.4` | 24,000 | 0 | 24,000 | 0 |

用 `certify --keep-samples 64` 量三种合并结果（无 trim / trim 1 / margin 1.5）：都是 **64/64 regressed**。
2.4 万 trits 的合成改动本身就足以毁掉 384 维行为，trim 只决定“合并哪些票”，不改变这条底线。

### XOR（2-2-1，oracle 4 样本）

三任务：A 把一格改成 `0`（|Δ|=1），B 同一格改成 `+`（|Δ|=2），C 改另一格。

| 场景 | changed | dropped | before_loss | after_loss | exact |
|---|---:|---:|---:|---:|---|
| 无 trim | 2 | 0 | 0 | 3 | false |
| `--trim 1` | 2 | 0 | 0 | 3 | false |
| `--trim-margin 2` | 0 | 2 | 0 | 0 | **true** |

两条可用的结论：

1. **margin 是保守旋钮**：门槛把低一致坐标整条丢掉，XOR 上直接回到 base 的正确行为；
2. **count-trim 不改善正确性**：它只改变“哪一票胜出”，在这个例子上结果与无 trim 相同。
   想让 trim 提升指标，需要的是“同一坐标上方向正确但幅度不同的票”，合成任务里没有。

## 警告：叠加顺序

`--trim 1 --trim-margin 1.5` 会把 24,000 个坐标**全部**丢掉（changed=0）。
原因：count-trim 先把每坐标压到一票，票和只剩 ±1/±2，`margin 1.5` 于是几乎全裁掉。
两个旋钮的阈值要按“裁剪后”的票和标定；需要保守时只用一个。

## 未证栏

- 没有真实多任务 HF 微调（合成任务是每任务 24,000 条确定性编辑），trim 的收益在真实任务算术上未测；
- 三值空间里 trim 只看 |Δ| 三档，粒度远粗于 f32 的 top-k%，与 TIES 原文的“幅度”不完全对应；
- margin 的门槛与任务数、权重尺度相关，没有自适应默认值；
- 与 `--tune-weights` 的交互未测（调权 + trim 的联合搜索没做）。

## 接口

- `TrimTop` / `TrimOptions`（`keep()`、`is_identity()`）；
- `merge_tasks_weighted_trim(base, tasks, weights, trim)`；
- `merge_tasks_weighted_trim_report(...) -> (Net, MergeReport)`；
- CLI `task-merge --trim K|ratio [--trim-margin M]`，输出 `trim_top{kind,value}` 与 `trim_margin`。
