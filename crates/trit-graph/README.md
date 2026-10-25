# trit-graph：三态结构基础层（M0）

给后面的三态图/树/超图模型（TriMPNN、Tri-TreeOracle、Tri-TD-GNN…）准备地基。
**不训练、不推理**，只提供"结构 + 三态"的最小语义与位运算原语。

## 数据结构

```rust
TritGraph
├── 节点：pos / neg 两个位集（都 0 = Unknown），另有 critical 掩码
└── 边  ：pos / neg / valid 三个位集
          valid=0 → 不存在；valid=1 且 pos/neg 都 0 → 中立
```

- 节点状态：`+1` 支持、`0` 未知、`-1` 反对（默认 `0`）。
- 边状态：`Some(+1)` 支持、`Some(-1)` 反对、`Some(0)` 中立、`None` 不存在。
- 邻接是**稠密位集**：`n × ceil(n/64)` 个 `u64`，每个节点三种状态各一份。

## API

| 方法 | 作用 | 复杂度 |
|---|---|---|
| `new(n)` / `set_node` / `node` | 建图与节点三态 | O(1) |
| `set_edge(u, v, Option<Ternary>)` / `edge` | 边的四种状态 | O(1) |
| `set_critical(node, bool)` | 关键节点（否定票可触发强否决） | O(1) |
| `for_each_out(u, f)` | 遍历 `u` 的存在出边（含状态） | O(n/64 + 出度) |
| `out_degree` / `out_counts` | 出度 / 三种边状态计数 | O(n/64) |
| `topological_order` / `is_dag` | Kahn 拓扑序；有环返回 `None` | O(n²/64 + E) |
| `dfs_tree(root)` | DFS 生成树（父指针 + 先序） | 同上 |
| `council(u)` | 三态议会：邻居投票合成一个判决 | O(出度) |

## 三态议会（M0 规则）

- 支持边投邻居节点的状态；反对边投其**反**；中立边弃权。
- 关键节点投出反对票 ⇒ **强否决**，直接 `-1`。
- 否则多数票：`+` 多判 `+`、`-` 多判 `-`、平票/全弃权判 `0`。

这条规则就是后面 TriMPNN 的"一层"；M1 会把它换成可学习的三态转移表。

## 例子

```rust
use trit_core::value::Ternary;
use trit_graph::{TritGraph, council};

let mut g = TritGraph::new(3);
g.set_node(1, Ternary::True);
g.set_node(2, Ternary::False);
g.set_edge(0, 1, Some(Ternary::True));   // 支持边 → 投 +1
g.set_edge(0, 2, Some(Ternary::True));   // 支持边 → 投 -1
assert_eq!(council(&g, 0).outcome, Ternary::Unknown); // 平票悬置

assert!(g.is_dag());
let tree = g.dfs_tree(0);
assert_eq!(tree.roots().collect::<Vec<_>>(), vec![0]);
```

## 限制（M0 刻意不做）

- **稠密邻接**：`n` 大了内存按 `n²` 涨；稀疏 CSR、超图、多重边都留到后续。
- 没有权重、节点特征、边类型；没有批处理。
- 三态议会是固定规则表，不可学习；没有 STE/训练。
- 拓扑序把所有存在的边都当方向边；三态语义在排序里不参与。

## 测试

```bash
cargo test -p trit-graph     # 15 个：多字符边界、拓扑序/环、DFS 树、议会各种否决/平票
```
