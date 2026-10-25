# trit-graph：三态结构基础层（M2）

给后面的三态图/树/超图模型（TriMPNN、Tri-TreeOracle、Tri-TD-GNN…）准备地基。
**不训练、不推理**，只提供"结构 + 三态"的最小语义与位运算原语。

M2 在 M1（稀疏 CSR + 1-WL）之上加了**三态超图**与**树分解容器**；议会规则抽成公共 `tally`，
超边、袋、图邻居共用一份计票。

## 数据结构

```rust
TritGraph
├── 节点：pos / neg 两个位集（都 0 = Unknown），另有 critical 掩码
└── 边  ：pos / neg / valid 三个位集
          valid=0 → 不存在；valid=1 且 pos/neg 都 0 → 中立
```

- 节点状态：`+1` 支持、`0` 未知、`-1` 反对（默认 `0`）。
- 边状态：`Some(+1)` 支持、`Some(-1)` 反对、`Some(0)` 中立、`None` 不存在。
- 邻接两种表示：
  - **稠密位集** `TritGraph`：`n × ceil(n/64)` 个 `u64`，每个节点三种状态各一份；
  - **稀疏 CSR** `TritCsr`：`n + nnz`，边表里存目标与三态，边少时内存线性增长。
- `TritHypergraph`：三态超边 = 成员位集 + 自身三态 + 关键位；
- `TreeDecomposition`：袋（节点位集）+ 袋树（父指针），带覆盖/运行交校验。

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
| `TritCsr::from_dense` | 稠密 → 稀疏（保留节点三态与关键位） | O(n + E) |
| `TritCsr::neighbors` | CSR 邻域迭代 | O(出度) |
| `wl_refine(view)` | 三态 1-WL 颜色细化 | O(轮数 × (n + E)) |
| `TritHypergraph::add_hyperedge` / `incident` / `verdict` | 超边、关联查询、超边议会 | 建边 O(成员)；关联 O(超边数) |
| `TreeDecomposition::validate` | 节点覆盖 / 边覆盖 / 运行交 | O(n + E + Σ袋) |
| `TreeDecomposition::bag_verdict` | 袋内三态议会 | O(袋大小) |

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

## 超图（M0 容器）

- 超边自己有三态与关键位；`verdict(id)` 对**成员节点**开议会（关键节点的反对票强否决）。
- 超边自身的三态不参与这次判决——它是给后续池化/超边间传播用的。
- M0 不提供超图→图的展开，也没有超边之间的依赖结构；这些是 Tri-HyperGNN 的事。

## 树分解（容器 + 校验）

- 只做容器：袋是节点位集，袋之间是一棵树；**不实现分解算法**。
- `validate(graph)` 查三条：
  1. 每个节点至少在某个袋里；
  2. 每条边被某个袋同时覆盖；
  3. 运行交（running intersection）：同一节点的所有袋在袋树上连通。
- `bag_verdict(i, graph)` 用袋内节点状态开议会（可给后续袋级判决做输入）。

## 三态 1-WL

初始颜色来自节点三态；每轮把 `(当前颜色, 邻居的 (边状态, 颜色) 多重集)` 压成新颜色，
划分稳定即停。**有向**版本：只看当前节点的出边。

- 能区分：路径 vs 星形（5 色 vs 2 色）、边符号不同（菱形把一片改成反对边 → 3 色变 4 色）。
- 区分不了：正则图（C3 与 C6 都是单色）——这是 1-WL 的已知上限，不是 bug。
- 颜色编号只在单图内有意义，不跨图比较。

## 限制（M2 仍然不做）

- **不做树分解算法**（只校验给定的袋树）；没有超图→图展开 / 超边间的消息传递。
- 没有权重、节点特征、批处理；稠密图仍是 `n²` 内存；CSR 只有出边。
- 三态议会是固定规则表，不可学习；没有 STE/训练。
- 拓扑序把所有存在的边都当方向边；三态语义在排序里不参与。

## 测试

```bash
cargo test -p trit-graph     # 35 个：稠密/稀疏一致性、跨字边界、拓扑/环、DFS、议会、WL、超图、树分解校验
```
