//! 三态结构基础层（M0）。
//!
//! 给后面的 "TriMPNN / Tri-TreeOracle / Tri-TD-GNN" 准备地基：
//! - [`TritGraph`]：**三态节点 + 三态边**（支持 / 反对 / 中立 / 不存在），位集邻接；
//! - [`Tree`]：从图上跑 DFS 得到的生成树（父指针 + DFS 先序 + 逆先序当后序）；
//! - [`council`]：三态议会——邻居投票合成一个三态判决（多数、强否决、平票悬置）。
//!
//! 边界（M0 刻意不做）：
//! - 邻接是**稠密位集**（n×n 位），适合千级节点；稀疏 CSR、权重、节点特征都不在 M0；
//! - 边只有四种状态，没有边类型/多重边/自环限制；
//! - 议会规则是**固定表**（多数 + 强否决），还不是可学习的三态转移表。

mod council;
mod graph;
mod tree;

pub use council::{Council, council};
pub use graph::TritGraph;
pub use tree::Tree;
