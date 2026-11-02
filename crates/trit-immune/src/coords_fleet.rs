//! 坐标层的 Fleet 方法面（实现见 [`crate::coords`]）。
//!
//! 从 `Fleet` 拆出（上游 module budget：单文件 ≤800 行）——这里只放
//! 「舰队怎么登记坐标」的固有方法，算法与账本格式都在 [`crate::coords`]。
//! 方法路径与 [`crate::fleet::Fleet`] 完全一致（同一类型的 impl 块可以
//! 分散在多个文件），调用方不需要知道这次拆分。

use crate::coords::{self, CoordDecl, CoordKey};
use crate::dag::NodeId;
use crate::fleet::{CrossRepoDelta, CrossRepoLink, Fleet};
use std::io;
use std::path::{Path, PathBuf};

impl Fleet {
    // ---- 依赖坐标（共享库在多项目下的身份；实现见 [`crate::coords`]）----

    /// 打开坐标注册表（追加式；声明影响裁决，账外无数据）。
    pub fn open_coord_ledger(&mut self, path: impl Into<PathBuf>) {
        self.coord_ledger = Some(path.into());
    }

    pub(crate) fn coord_ledger_path(&self) -> Option<PathBuf> {
        self.coord_ledger.clone()
    }

    pub(crate) fn push_provider(&mut self, d: CoordDecl) {
        self.provides.push(d);
    }

    /// 把已在 DAG 里的坐标节点登记进声明表（不重复登记）。
    pub(crate) fn adopt_use(&mut self, repo: &str, node: NodeId, coord: &CoordKey) {
        if !self
            .uses
            .iter()
            .any(|d| d.repo == repo && d.coord.key() == coord.key())
        {
            self.uses.push(CoordDecl {
                repo: repo.to_string(),
                node,
                coord: coord.clone(),
            });
        }
    }

    pub fn provides(&self) -> &[CoordDecl] {
        &self.provides
    }

    pub fn uses(&self) -> &[CoordDecl] {
        &self.uses
    }

    /// 声明某仓某节点是某个依赖坐标的裁决出处。
    pub fn declare_provider(
        &mut self,
        repo: &str,
        node: NodeId,
        coord: CoordKey,
    ) -> io::Result<()> {
        coords::declare_provider(self, repo, node, coord)
    }

    /// 把一份 `Cargo.lock` 入账为该仓的消费坐标节点（幂等；`build` 见 [`crate::coords`]）。
    pub fn attach_lock(
        &mut self,
        repo: &str,
        lock_text: &str,
        build: Option<&str>,
    ) -> io::Result<coords::AttachReport> {
        coords::attach_lock(self, repo, lock_text, build)
    }

    /// 坐标派生边（精确匹配才连；不落注册表，是声明的确定性视图）。
    pub fn coord_edges(&self) -> Vec<CrossRepoLink> {
        coords::derive_edges(&self.provides, &self.uses)
    }

    /// **全部跨仓边** = 人工/接口声明的边 ⊕ 坐标派生的边。
    /// 传播视图（[`crate::fleet::Fleet::contamination`]）、渲染、攻击链、落地影响都读这一个口径——
    /// 免得坐标边只在落账时生效、查询视图却看不见（两套真相）。
    pub fn cross_edges(&self) -> Vec<CrossRepoLink> {
        let mut v = self.cross_links().to_vec();
        v.extend(self.coord_edges());
        v
    }

    /// 坐标级跨仓落账（语义与 [`crate::fleet::Fleet::sync_cross_repo`] 同一套纪律，
    /// 只是边来自坐标声明）。
    pub fn sync_coordinates(&mut self, note: &str) -> io::Result<Vec<CrossRepoDelta>> {
        coords::sync(self, note)
    }

    /// 坐标缺口清单（每一条都是 0 态候选：**不匹配不静默**）。
    pub fn coord_gaps(&self) -> Vec<coords::CoordGap> {
        coords::gaps(self, &self.provides, &self.uses)
    }

    /// 坐标视图（TSV：PROV/USE/GAP 三段，供 HTTP 与 CLI 直读）。
    pub fn render_coords(&self) -> String {
        coords::render(self)
    }

    /// 从坐标注册表读回声明（追加式；加载期不验证，失效声明由
    /// [`crate::fleet::Fleet::coord_gaps`] 上报）。
    /// 返回 (提供者数, 消费者数, 坏行数)。已存在的 (仓, 坐标键) 不重复入账。
    pub fn load_coords(&mut self, path: &Path) -> io::Result<(usize, usize, usize)> {
        let (provides, uses, bad) = coords::load_lines(path)?;
        let mut np = 0;
        let mut nu = 0;
        for d in provides {
            if self.provides.iter().any(|e| e == &d) {
                continue;
            }
            self.provides.push(d);
            np += 1;
        }
        for d in uses {
            if self
                .uses
                .iter()
                .any(|e| e.repo == d.repo && e.coord.key() == d.coord.key())
            {
                continue;
            }
            self.uses.push(d);
            nu += 1;
        }
        Ok((np, nu, bad))
    }
}
