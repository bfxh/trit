# trit 仓库规则（智能体必读）

## 死命令（2026-10-29 用户口述）

1. **这是用户自己的 Rust 三进制工程**：模型、训练、推理、工具一律在本仓用 Rust 三进制实现。
2. **缺实现就自己造**；不许拿 Python 训练、外部模型或二进制现成件顶替。Python 只作对齐基线，不交付。
3. **模型权重与训练产物属于本仓**：代码进 `crates/`，大件进 `D:\KF\trit-data\`，不许放在 `.yanagent/`。
4. **没做完必须当场说明缺口**，不得把"文档里写了规矩"当成实现。

## 工程口径

- 实验层 `publish = false`；单文件 ≤800 行（`python scripts/module_budget.py`，台账只减不增）。
- 收工一条命令：`cargo test --workspace` + `python scripts/module_budget.py`。
- 训练层在 `crates/trit-train`（STE + 位平面 ALU）；新模型优先复用同一套量化/STE 口径。
- 三进制结论按 `docs/TERNARY-BOUNDARIES.md` 归 A/B/C/D，且必须带二进制基线。
