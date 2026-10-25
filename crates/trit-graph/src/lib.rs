//! 三态结构基础层（M5）。
//!
//! 给后面的 "Tri-TreeOracle / Tri-TD-GNN / Tri-HyperGNN" 准备地基：
//! - [`TritGraph`]：稠密位集图（三态节点 + 四态边 + 关键位）；
//! - [`TritCsr`]：同一语义的稀疏 CSR，`n + nnz` 存储；
//! - [`TritHypergraph`]：三态超边（成员位集）+ 超边议会；
//! - [`TreeDecomposition`]：树分解容器（袋 + 袋树 + 覆盖/运行交校验）；
//! - [`Tree`]：DFS 生成树（父指针 + 先序 + 逆先序当后序）；
//! - [`council`] / [`tally`]：三态议会（多数、强否决、平票悬置）；
//! - [`TriMpnn`] / [`TransitionTable`]：可学习三态转移表 + 一层消息传递（STE）；
//! - [`wl_refine`]：三态 1-WL 颜色细化；
//! - [`wl_subtree_kernel`] / [`path_kernel`]：三态图核；
//! - [`tri_pagerank`] / [`TriScore`]：三通道随机游走 / PageRank。
//!
//! 共享逻辑都在 [`TriGraphView`] 上：稠密/稀疏只实现四个基本方法，
//! 拓扑序、DFS 树、计数、议会、WL、图核、随机游走、消息传递从 trait 派生，
//! 避免两套实现分叉。
//!
//! 边界（仍然不做）：
//! - 没有权重、节点特征、批处理；树分解只做容器与校验，**不做分解算法**；
//! - TriMPNN 只做一层传播 + 表级监督拟合，整图标签的端到端训练还没做；
//! - 路径核只到长度 4；1-WL 与 WL 子树核区分不了正则图（已知上限）。

mod council;
mod csr;
mod decomp;
mod graph;
mod hyper;
mod kernel;
mod mpnn;
mod nodes;
mod tree;
mod view;
mod walk;
mod wl;

pub use council::{Council, council, tally};
pub use csr::TritCsr;
pub use decomp::TreeDecomposition;
pub use graph::TritGraph;
pub use hyper::{Hyperedge, TritHypergraph};
pub use kernel::{normalized, path_kernel, wl_subtree_kernel};
pub use mpnn::{TransitionTable, TriMpnn};
pub use tree::Tree;
pub use view::TriGraphView;
pub use walk::{TriScore, tri_pagerank};
pub use wl::{color_count, wl_refine};
