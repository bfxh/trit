//! 存盘（从 tconv/io.rs 下沉）：读外部文本那半段要加尺寸校验、得腾预算，
//! 而这里只是纯写出，拆开之后各留各的上限。

use super::super::net::Net;

impl Net {
    /// 存盘（文本，`TPC1`）：层维度 + 权重 + bias + scale，再跟 BN 的 γ/β。
    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        use std::fmt::Write as _;
        let mut out = String::new();
        let _ = writeln!(
            out,
            "TPC7 {} {} {} {} {} {} {} {}",
            self.layers.len(),
            self.cond,
            self.linear_out as u8,
            self.circular as u8,
            self.highpass,
            self.nk,
            self.heads.len(),
            self.layers.first().map(|l| l.dil).unwrap_or(1)
        );
        for l in &self.layers {
            let _ = writeln!(out, "{} {}", l.cin, l.cout);
            for v in [&l.w, &l.b, &l.scale] {
                for x in v.iter() {
                    let _ = write!(out, "{x} ");
                }
                let _ = writeln!(out);
            }
        }
        let nbn = self.bns.iter().filter(|b| b.is_some()).count();
        let _ = writeln!(out, "BN {nbn}");
        for (i, bn) in self.bns.iter().take(self.layers.len()).enumerate() {
            match bn {
                Some(b) => {
                    let _ = writeln!(out, "{i} 1");
                    for v in [&b.gamma, &b.beta] {
                        for x in v.iter() {
                            let _ = write!(out, "{x} ");
                        }
                        let _ = writeln!(out);
                    }
                }
                None => {
                    let _ = writeln!(out, "{i} 0");
                }
            }
        }
        // FiLM：层号 + 是否启用 + (nk+1)×c 的 g/b
        let nfilm = self.films.iter().filter(|f| f.is_some()).count();
        let _ = writeln!(out, "FILM {nfilm}");
        for (i, f) in self.films.iter().enumerate() {
            match f {
                Some(ff) => {
                    let _ = writeln!(out, "{i} {} {}", ff.nk, ff.c);
                    for v in [&ff.g, &ff.b] {
                        for x in v.iter() {
                            let _ = write!(out, "{x} ");
                        }
                        let _ = writeln!(out);
                    }
                }
                None => {
                    let _ = writeln!(out, "{i} 0 0");
                }
            }
        }
        // 输出头：数量 + 每个头（启用=1 + cin/cout + w/b/scale）
        let _ = writeln!(out, "HEADS {}", self.heads.len());
        for h in &self.heads {
            match h {
                Some(c) => {
                    let _ = writeln!(out, "1 {} {}", c.cin, c.cout);
                    for v in [&c.w, &c.b, &c.scale] {
                        for x in v.iter() {
                            let _ = write!(out, "{x} ");
                        }
                        let _ = writeln!(out);
                    }
                }
                None => {
                    let _ = writeln!(out, "0 0 0");
                }
            }
        }
        std::fs::write(path, out).map_err(|e| format!("{}: {e}", path.display()))
    }
}
