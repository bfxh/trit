# 权重空间编程：论文地图与任务清单

> 说明：下面的 arXiv 号/会议/结论是调研阶段按公开文献整理，**未逐篇在线核对原文**；
> 落地前应取 PDF 校一遍。代码侧“已实现/缺口”以本仓 `crates/trit-wspace/README.md`
> 技术树与 `src/lib.rs` 模块表为准。

## 1. 论文地图（按主题）

### 1.1 三值 / 1-bit LLM 与量化

| 论文 | 结论 / 可用点 |
|---|---|
| BitNet b1.58, arXiv:2402.17764 | 三值权重 + 8-bit 激活**从头训练**可追平 f32 LLaMA；RTN 直接三值化会崩。 |
| BitNet a4.8, arXiv:2411.04965 | 4-bit 激活需要混合精度（outlier 通道）+ 激活稀疏 + 双分支，纯三值不够。 |
| TWN, arXiv:1612.01064 | 阈值 `Δ≈0.7·E|w|` 与 absmean-round 的 0.5 阈值不同；scale 乘子是免费收益。 |
| TTQ, arXiv:1605.04711 | 权重/激活各一个 scale，per-channel 更好。 |
| GPTQ, arXiv:2210.17323 | 逐列量化 + Hessian/OBS 误差补偿，是免训练 PTQ 的最强抓手。 |
| AWQ, arXiv:2306.00978 | 激活感知的显著列缩放，网格搜 α。 |
| SmoothQuant, arXiv:2211.10438 | 把激活 outlier 迁移到权重侧（α=0.5）。 |
| LLM.int8(), arXiv:2208.07339 | outlier 是系统性的（~0.1% 通道、~20× 幅值），必须留 fp16。 |
| BiLLM, arXiv:2402.04291 / ARB-LLM, arXiv:2402.11988 / TernaryLLM, arXiv:2310.17157 | 残差/低秩/稀疏补丁是“三值主干 + 少量 f32”的现实路线。 |
| ParetoQ, arXiv:2403.16971 | 极低比特下三值可能优于 2-bit；scale 粒度/学习是关键。 |

### 1.2 模型编辑 / 知识编辑

| 论文 | 结论 / 可用点 |
|---|---|
| ROME, arXiv:2202.05262 | causal tracing 定位中层 MLP，再秩一更新；副作用只抽样评测。 |
| MEMIT, arXiv:2210.07229 | 跨层最小二乘批量编辑上千事实；仍无副作用保证。 |
| PMET, arXiv:2308.08742 | 连 MHSA 的 V/O 一起解；更精确但仍是最小二乘。 |
| AlphaEdit, arXiv:2410.02355 | 把更新投影到保持知识零空间，理论上界住旧知识扰动——最接近“副作用可证”。 |
| WISE, arXiv:2405.14768 | ROME 类不能终身编辑，需要外部显式记忆；反向证据。 |
| Diff Pruning, arXiv:2010.10499 | 学二值 diff 掩码（L0 + STE）只改少数参数——离散稀疏编辑的先例。 |

### 1.3 模型合并 / 任务算术

| 论文 | 结论 / 可用点 |
|---|---|
| Task Arithmetic, arXiv:2212.04089 | `τ=θ_ft−θ_pre`，加/减/类比；三值只剩符号与 λ。 |
| TIES-Merging, arXiv:2306.01708 | trim 小幅度 + elect sign + disjoint merge；**trim 是收益主因**。 |
| DARE, arXiv:2311.03099 | 随机丢 90–99% Δ 再 `1/(1−p)` 重标度；三值版重标度必须进 scale。 |
| Git Re-Basin, arXiv:2209.04836 | 消隐藏单元置换后再合并；也是置换水印的攻击。 |
| Model Soups, arXiv:2203.05482 / Task Singular Vectors, arXiv:2412.00081 / Model Stock, arXiv:2403.19522 | 平均类方法在三值上退化，只剩投票/scale。 |

### 1.4 可解释性 / 因果 / 水印

| 论文 | 结论 / 可用点 |
|---|---|
| Causal tracing (ROME) | corrupt→denoise→mediation；有界域上可做精确归因。 |
| Attribution Patching / AtP*, arXiv:2403.00745 | 线性近似梯度定位，给大网省前向。 |
| Causal Scrubbing (Chan et al., 2022) | 严格假设检验；本仓 Oracle 穷举是其强形式。 |
| Probing, arXiv:2102.12452 | 权重/激活探针；本仓 `probe`/`localize` 已做行级。 |
| Uchida et al., arXiv:1701.03077 / DeepMarks, arXiv:1901.07744 | 权值水印 + z-test；三值可升级为 bit 精确校验。 |
| IPGuard, USENIX Sec'19 / Conferrable Adv. Examples, arXiv:1912.01805 | 行为指纹/挑战集；本仓 audit 行为链已有底座。 |

## 2. 对照本仓：已实现与缺口

已实现（技术树 L0–L7）：WSP1/2/3/4、块索引、tile、Patch/Journal/rebase/audit、Oracle、
repair/propose/analyze/explain/probe/localize、merge/task/DARE、safetensors、GGUF、
CLI/MCP、learned、gradient、写锁、GPU WF32、6 层读数。

关键缺口：

- 没有 **f32 微调差分 → 三值补丁** 的桥（外部 HF 结果进不来）。
- 没有 **保持集副作用证书**（只测“改了会怎样”，不证明“没改坏什么”）。
- TIES 缺 **trim**；DARE 的 `1/(1−p)` 在三值下对符号行为恒等（需文档钉死）。
- 没有 **真实激活校准**（bias_e2e 仍用 uniform 随机输入）。
- 三值化只有 per-row absmean；缺 per-group/GPTQ/AWQ/残差补丁。
- 没有 **水印/指纹** 模块；置换密钥、挑战集指纹都未做。
- 没有 **SAT/BDD 行为等价证明**（本仓独有底座，能推翻“大网只能抽样”）。
- 没有真实 transformer 前向（attention/LN/GELU/残差），6 层仍是线性段。
- WSP3/4 不能随机读；GGUF 与 llama.cpp 未互操作；MCP 未覆盖新命令。

## 3. 任务清单（按价值/成本排序）

### P0：先做（1–2 周量级）

1. **`quantize-delta`：f32 ΔW → 三值最小补丁桥**（本轮落地）。
   - 读 base/tuned safetensors → 分别三值化 → 逐点 diff → 按 `|ΔW|` top-k 保留。
   - 产出 `Patch`，可直接走 `Patch/Journal/AuditLog/rebase`；`--oracle` 对比
     before/full/trim loss。
   - 价值：把外部 HF 微调/任务向量接进权重空间编程；低成本。
2. **`certify`：保持集副作用证书**（AlphaEdit 的离散精确版）。
   - 输入 patch + `preserve` oracle；应用前后逐样本比较，输出 violations。
   - 有界域上是**完全可判定**，不是抽样；补上“保证不会改坏”的一侧。
3. **TIES trim**：`task_merge --trim k / --trim-margin m`，按票数/票差裁剪；
   `--tie keep|zero` 对齐 disjoint merge。
4. **真实激活校准**：把 bias_e2e/三值化读数的输入从 `uniform(-1,1)` 换成
   HF 一次 forward 的真实激活（或小语料统计）；先钉住“随机输入低估 1.5–3×”。
5. **三值化 scale 乘子扫描 / per-group scale**：`s/E|w| ∈ [0.6,1.6]` 与 g=64/128；
   直接对 bias_e2e 的 `rel_l2` 出读数。

### P1：中期

6. **GPTQ 式逐列补偿三值化**：H=XᵀX、Cholesky、按 diag(H) 排序逐列量化。
7. **残差低秩/fp16 侧车**：`R=W−s·Q` 保留 rank≤4 或 top-k 列；三值主干 + 少量 f32。
8. **批量编辑干扰矩阵**：一次提交 k 条编辑，输出 k×k 影响并贪心重排。
9. **序列编辑遗忘曲线**：audit 对全部历史 oracle 的保持率，对齐 WISE/MEMIT。
10. **激活级因果追踪**：corrupt 全域后逐单元 restore，输出 per-layer restoration。
11. **挑战集行为指纹 CLI**：`fingerprint emit|check`，落到 audit 行为链。
12. **零活跃 trit 水印**：`localize` 筛免影响位置 → 改少量 trit → Oracle 证明行为不变 →
    bit 精确提取；重复码/纠错。
13. **置换密钥指纹 + 对齐攻击自测**：`align --exact` Hungarian，加 `perm-attack` 用例。
14. **学习式提议器升级**：把 `|∂L/∂w|` 加入特征（第 17 维）或上 1 隐层 MLP。

### P2：长期 / 大工程

15. **SAT/BDD 行为等价证明**：把 `Net` 编码进 `trit-sat`/`trit-bdd`，证明编辑前后
    差异集为空/非空；本仓独有筹码。
16. **BitNet b1.58 真实权重定位读数**：官方三值权重上跑 probe/localize。
17. **WSP4 分块随机访问（WSP5）**：每块独立码表，牺牲压缩率换随机读。
18. **llama.cpp `TYPE_TRIT` 互操作**：转换器/上游补丁，实测加载。
19. **MCP 工具补齐**：tile/index/lock/learned/gguf/quantize-delta 进 schema。
20. **真实 transformer 最小前向**：attention/LN/GELU/残差，接 HF 权重，替换 6 层线性段。
21. **水印抗合并/抗剪枝评估**：重训、剪枝、对齐后的提取率。
22. **多任务连续编辑**：无遗忘的序列编辑协议 + 回滚。
23. **三值蒸馏到真实 LLM**：QAT/STE + 真实任务指标。
24. **模型库/格式注册**：safetensors/GGUF/PyTorch state_dict 的统一清单。

## 4. 本轮落地：P0-1 `quantize-delta`

`delta::diff_weights` + `examples/quantize_delta.rs`：

```bash
cargo run --release -p trit-wspace --example quantize_delta -- \
  --base base.safetensors --tuned tuned.safetensors --tensor dense.weight \
  --trim 0.2 --patch-out patch.json --out edited.wsp --oracle oracle.json
```

- base/tuned 各自 absmean 三值化；只对三值变化的坐标生成 `SetTrit`/`SetBias`。
- `--trim` 按 `|tuned - base|` 保留 top 比例；scale 保留 base。
- 输出 patch JSON 与 apply 后的 WSP1；`--oracle` 报告 before/full/trim loss。

真实读数（all-MiniLM-L6-v2 `query` 384×384，合成 Δ：每 10 个元素取反）：

| 项 | 值 |
|---|---:|
| 三值变化 | 10,128 trits |
| trim 0.2 保留 | 2,026 |
| 丢弃 | 8,102 |
| patch JSON | 215,381 B |
| edited WSP1 | 301,616 B |

未证：合成 Δ 不是真实 HF 微调；scale 不迁移；没有匹配 384 维输出的真实任务 oracle。

其余 P0–P2 按上表逐项推进。
