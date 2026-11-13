# trit-wspace 未证栏（汇总）

把各结果文档的“未证栏”集中到一处，避免只写在单个文件末尾。每条给来源与补证所需。

## 1. 真实模型 / 任务指标

| 项 | 现状 | 补证 |
|---|---|---|
| f32 bias 的端到端收益 | `bias_e2e` 只用随机 `uniform(-1,1)` 输入，测 MiniLM 第 0 层线性三段；级联 cosine ~0.44 | tokenizer + 真实激活跑完整 6 层；在检索/分类任务上测 |
| WSP4 的更大模型 | 只在 all-MiniLM-L6-v2（103 张量，5,985,699 B）上测；82.7% of WSP2 | 更大模型/GPU；不同稀疏度 |
| DARE 块级重标度 | `dare` 已实现坐标/行/层/块丢弃与报告；块级重标度细节未证 | 在真实任务向量上对拍 |
| 学习式提议器 | 4→8→1 随机网络 + 采样 oracle；单工件内留出编辑 | 跨任务/跨工件留出；真实任务 oracle |
| 真实任务精度 | 本 crate 没有 MATH-500/检索等任务读数 | 接真实 f32 模型 → 三值权重 → 任务指标 |

## 2. 容器 / 互操作

| 项 | 现状 | 补证 |
|---|---|---|
| GGUF ↔ llama.cpp | `TYPE_TRIT=100` 是自定义类型，上游不认；张量名未映射 `blk.N.*` | 在 llama.cpp 侧加类型或转换器；实测加载 |
| GGUF 不保留 f32 bias 幅度 | 103 张量往返 67/103 逐字节一致；36 个带 `biasf` 的 `.weight` 只差 biasf/checksum | 扩展 TYPE_TRIT 数据段或旁车文件 |
| WSP3/WSP4 随机访问 | 两者是整段压缩流，不能按块随机读 | 分块压缩（每块独立码表），接受压缩率损失 |
| WSP2 索引 | 固定 64 行块；未接 mmap；默认只查长度 + 尾部校验和 | 自适应块；mmap；读时全量校验 |
| WSPM 整模型容器 | blob 全量读入内存，无流式/mmap 与按张量随机读；清单的 `skipped`/原始 `source` 不进容器（重建 manifest 的 `source` 指向容器自身）；MCP 未接 | 流式打包；与 `read-block` 串联做按张量读；MCP 工具 |
| GPU 读数 | `trit-gpu` 的 `wspace_gpu` 示例只测 WF32 matmul（512 批量 6.6×、intermediate 4.1×） | GPU 接进 `analyze`/`repair` 的候选批量评估（需要批量 API + 依赖边界设计） |

## 3. 编辑 / 事务 / 并发

| 项 | 现状 | 补证 |
|---|---|---|
| tile 编辑 | 只支持矩形块、只改 trits；无跨层 copy/swap | 非矩形 mask；跨层搬运；tile 级 diff |
| 并发写锁 | advisory；只约束 CLI `run`；MCP 未接；跨主机/NFS 未证；stale 10 分钟 | MCP 接入；NFS 锁语义；长命令锁续期 |
| 审计日志 | `patch --log` 追加模式；重复应用会产生重复条 | 幂等键/去重；并发追加锁已覆盖 CLI |

## 4. 发布 / 实现边界

| 项 | 现状 | 补证 |
|---|---|---|
| `trit-codec` 实验层 | `publish = false`；`publish_graph` 禁止可发布依赖实验层 | 转正或拆出可发布子 crate |
| MCP 工具面 | 21 个工具；tile/index/lock/learned/gguf 未全部进 MCP | 补 MCP 工具与 schema |

## 5. 怎么加新读数

- 每个结果文档的“未证栏”必须同步到这里（或至少链接）。
- 新读数要带：可复算命令、输入分布、样本数、随机种子、机器/平台。
- 负结果也要写（WSP3/rANS 已按此处理）。
