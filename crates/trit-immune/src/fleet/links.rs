//! 跨仓污染边的注册表面：声明、追加式落盘、加载与读取。
//!
//! 从 `fleet.rs` 拆出来只为一件事：**跨仓边影响裁决，所以它必须有一套自己的账**
//! （"账外无数据"）。方法仍挂在同一个 `Fleet` 身份上——`impl Fleet` 可以分散在多个模块，
//! 私有字段对本模块可见（本模块是 `fleet` 的后代）。

use crate::dag::NodeId;
use crate::delta::{esc, unesc};
use crate::trit::Trit;
use std::io;
use std::path::{Path, PathBuf};

use super::{CrossRepoLink, Fleet};

impl Fleet {
    /// 打开跨仓边注册表（追加式；"账外无数据"——跨仓边影响裁决，必须入账）。
    pub fn open_link_ledger(&mut self, path: impl Into<PathBuf>) {
        self.link_ledger = Some(path.into());
    }

    /// 声明一条跨仓污染边（两端仓与节点必须已存在）。
    pub fn add_cross_link(
        &mut self,
        from: (&str, NodeId),
        to: (&str, NodeId),
        pass: Trit,
        note: &str,
    ) -> Result<(), String> {
        let (fr, fid) = from;
        let (tr, tid) = to;
        let ja = self.get(fr).ok_or_else(|| format!("源仓 {fr} 不存在"))?;
        if fid.0 as usize >= ja.dag().nodes().len() {
            return Err(format!("源节点 #{:02} 不存在于 {fr}", fid.0));
        }
        let jb = self.get(tr).ok_or_else(|| format!("目标仓 {tr} 不存在"))?;
        if tid.0 as usize >= jb.dag().nodes().len() {
            return Err(format!("目标节点 #{:02} 不存在于 {tr}", tid.0));
        }
        let link = CrossRepoLink {
            from: (fr.to_string(), fid),
            to: (tr.to_string(), tid),
            pass,
            note: note.to_string(),
        };
        if let Some(ledger) = &self.link_ledger {
            let line = format!(
                "FLEETLINK\t{}\t{}\t{}\t{}\t{}\t{}\n",
                esc(fr),
                fid.0,
                esc(tr),
                tid.0,
                pass,
                esc(note)
            );
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(ledger)
                .map_err(|e| e.to_string())?;
            use std::io::Write as _;
            f.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
        }
        self.cross_links.push(link);
        Ok(())
    }

    /// 从注册表载入跨仓边（追加式：只增；未知仓/节点的边照收，
    /// 传播时跳过——加载期不验证，注册表与仓的状态解耦）。
    pub fn load_links(&mut self, path: &Path) -> io::Result<(usize, usize)> {
        let text = std::fs::read_to_string(path)?;
        let (mut loaded, mut skipped) = (0usize, 0usize);
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let mut f = line.split('\t');
            match (
                f.next(),
                f.next(),
                f.next(),
                f.next(),
                f.next(),
                f.next(),
                f.next(),
                f.next(),
            ) {
                (
                    Some("FLEETLINK"),
                    Some(fr),
                    Some(fid),
                    Some(tr),
                    Some(tid),
                    Some(pass),
                    Some(note),
                    None,
                ) => {
                    let parsed = (|| -> io::Result<CrossRepoLink> {
                        let fr =
                            unesc(fr).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                        let fid = NodeId(
                            fid.parse::<u32>()
                                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
                        );
                        let tr =
                            unesc(tr).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                        let tid = NodeId(
                            tid.parse::<u32>()
                                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
                        );
                        let pass = pass
                            .parse::<Trit>()
                            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                        let note = unesc(note)
                            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                        Ok(CrossRepoLink {
                            from: (fr, fid),
                            to: (tr, tid),
                            pass,
                            note,
                        })
                    })()
                    .map_err(|e| {
                        io::Error::new(io::ErrorKind::InvalidData, format!("注册表行不合法：{e}"))
                    })?;
                    self.cross_links.push(parsed);
                    loaded += 1;
                }
                _ => skipped += 1,
            }
        }
        Ok((loaded, skipped))
    }

    pub fn cross_links(&self) -> &[CrossRepoLink] {
        &self.cross_links
    }
}
