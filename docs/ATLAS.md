# 组件地图：前端 / 后端 / 语义层 / 横切工具

> 扫描体系怎么用这张图：
> - **产物面**（`trit-redteam binary`）：`side` 字段判定产物归属——
>   `.wasm` / HTML = **前端产物**，PE / ELF = **后端（原生）产物**；
>   泄密/路径检查对两类产物都跑（前端字节码里嵌 API key 是真实威胁）。
> - **网络面**（`trit-procscan run` → `network_roles`）：采样窗口内
>   **监听 = 在当后端**、**发起连接 = 在当前端**、两者都有 = 双角色；
>   这是行为角色，不是进程的永久身份。
> - **源码面**（`scripts/hazard_gate.py`）：放行清单只覆盖**执行后端内核**
>   （上限来源在那里）——前端/语义层的 crate 出现裸指针类匹配默认即红。

## 分层地图（28 crate + playground）

### 浏览器前端

| 组件 | 证据 | 说明 |
|---|---|---|
| `playground/` | 纯 HTML/JS，无构建 | 网页 UI：在浏览器里求值/出真值表/查等价 |
| `trit-wasm` | 依赖 `wasm-bindgen`；只导出求值面 | 给 playground 的 WASM 绑定（编译目标 `wasm32`） |

### 语言前端（把文本变成结构）

| 组件 | 证据 | 说明 |
|---|---|---|
| `trit-core`（Parser/AST 部分） | `ast.rs` + `parser` 模块 | `.tri` 文本 → 表达式树；`Ternary`/位并行是前后端共用的内核 |
| `trit-lsp` | LSP JSON-RPC / stdio 子进程 | 编辑器服务：诊断（含深层三值 lint）、悬停真值表、导航 |

### 编译后端（把结构变成产物）

| 组件 | 证据 | 说明 |
|---|---|---|
| `trit-build` | 产出 `truth.tritbin` / `lookup.c` / `design.v` | 化简 + 精确复核 + 多目标产物 |
| `trit-circuit` | Verilog 导出模块 | 网表级：X 传播、ATPG、技术映射 |

### 执行后端（数据/产物的运行内核）

| 组件 | 证据 | 说明 |
|---|---|---|
| `trit-ops` | 裸指针 + AVX 内核（hazard gate 放行清单） | CPU 流式内核：掩码/查表，15.8× f32 |
| `trit-dot` | POPCNT 位并行（hazard gate 放行清单） | XNOR+POPC 点积与块稀疏 |
| `trit-gpu` | WGSL 内核（`--features gpu`） | GPU 三值矩阵乘 |
| `trit-codec` / `trit-texture` / `trit-float` | 纯库 | 编码 / 资产容器（GF(3) CRC）/ 三值浮点 |

### 语义层（前后端共用的真值语义）

`trit-eval`（求值/真值表/等价）、`trit-sat`（SAT 判定）、`trit-bdd`（规范形）、
`trit-policy`（决策表）、`trit-schedule`（三态调度）、`trit-belnap`（四值 opt-in）。

### 实验层（训练与模型，产出三值权重）

`trit-learn`（量化/STE/组件基座）、`trit-train`（QAT 线性层）、`trit-net`（三值网络 + TRN1）、
`trit-attn`（可训练注意力块）、`trit-graph`（三态图/TriMPNN）、`trit-machines`（SNN/元胞/分级回路）。

### 横切安全工具（挖自己用的）

`trit-unsafe`（K3 审 unsafe）、`trit-redteam`（产物红队：硬化/泄密/双导轨/fuzz/侧信道）、
`trit-procscan`（动态行为扫描 + 网络角色/追溯）、`scripts/hazard_gate.py`（并发/内存高危门）。

### 入口与伞包

`trit-cli`（`trit` 命令行）、`sansheng`（统一 re-export）。

## 边界（不猜）

- `trit-core` 同时是语言前端（解析器）与前后端共享内核——按模块而不是整个 crate 归层；
- 「前端」在本仓指**浏览器面**（playground/WASM），不是 GUI 桌面；
- 产物面识别只看魔数/文本特征（`\0asm`、`<!DOCTYPE html`、PE/ELF 头），
  改名伪装不在识别范围（那是 `attrib` 归因的活）。
