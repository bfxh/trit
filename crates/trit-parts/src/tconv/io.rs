use super::bn::Bn;
use super::conv::Conv;
use super::net::Net;

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
        for i in 0..self.layers.len() {
            match &self.bns[i] {
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

    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let txt = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        fn take<'a>(
            it: &mut impl Iterator<Item = &'a str>,
            path: &std::path::Path,
            what: &str,
        ) -> Result<f32, String> {
            it.next()
                .ok_or_else(|| format!("{}: 缺 {what}", path.display()))?
                .parse::<f32>()
                .map_err(|e| format!("{}: {what} 解析失败 {e}", path.display()))
        }
        let mut it = txt.split_whitespace();
        let head = it.next();
        if !matches!(
            head,
            Some("TPC1")
                | Some("TPC2")
                | Some("TPC3")
                | Some("TPC4")
                | Some("TPC5")
                | Some("TPC6")
                | Some("TPC7")
        ) {
            return Err(format!("{}: 不是 TPC1..TPC7", path.display()));
        }
        let nl = take(&mut it, path, "层数")? as usize;
        let cond = take(&mut it, path, "cond")? as usize;
        let has_linear = matches!(
            head,
            Some("TPC2") | Some("TPC3") | Some("TPC4") | Some("TPC5") | Some("TPC6") | Some("TPC7")
        );
        let linear_out = has_linear && take(&mut it, path, "linear")? != 0.0;
        let has_circ = matches!(
            head,
            Some("TPC3") | Some("TPC4") | Some("TPC5") | Some("TPC6") | Some("TPC7")
        );
        let circular = has_circ && take(&mut it, path, "circular")? != 0.0;
        let has_hp = matches!(
            head,
            Some("TPC4") | Some("TPC5") | Some("TPC6") | Some("TPC7")
        );
        let highpass = if has_hp {
            take(&mut it, path, "highpass")?
        } else {
            0.0
        };
        let nk_hdr = if matches!(head, Some("TPC5") | Some("TPC6") | Some("TPC7")) {
            take(&mut it, path, "nk")? as usize
        } else {
            0
        };
        let nheads = if matches!(head, Some("TPC6") | Some("TPC7")) {
            take(&mut it, path, "nheads")? as usize
        } else {
            0
        };
        let dil = if head == Some("TPC7") {
            (take(&mut it, path, "dilation")? as usize).max(1)
        } else {
            1
        };
        let mut layers = Vec::with_capacity(nl);
        for _ in 0..nl {
            let cin = take(&mut it, path, "cin")? as usize;
            let cout = take(&mut it, path, "cout")? as usize;
            let mut l = Conv::new(cin, cout, 1);
            for v in l.w.iter_mut() {
                *v = take(&mut it, path, "w")?;
            }
            for v in l.b.iter_mut() {
                *v = take(&mut it, path, "b")?;
            }
            for v in l.scale.iter_mut() {
                *v = take(&mut it, path, "scale")?;
            }
            layers.push(l);
        }
        let mut bns: Vec<Option<Bn>> = (0..nl).map(|_| None).collect();
        if it.next() != Some("BN") {
            return Err(format!("{}: 缺 BN 段", path.display()));
        }
        let _n = take(&mut it, path, "BN 数量")? as usize;
        for _ in 0..nl {
            let idx = take(&mut it, path, "BN 层号")? as usize;
            let on = take(&mut it, path, "BN 开关")? != 0.0;
            if !on {
                continue;
            }
            let c = layers[idx].cout;
            let mut b = Bn::new(c);
            for v in b.gamma.iter_mut() {
                *v = take(&mut it, path, "gamma")?;
            }
            for v in b.beta.iter_mut() {
                *v = take(&mut it, path, "beta")?;
            }
            bns[idx] = Some(b);
        }
        let mut net = Net {
            layers,
            bns,
            cond,
            linear_out,
            circular: false,
            highpass,
            films: (0..nl).map(|_| None).collect(),
            nk: 0,
            film_lr_mul: 1.0,
            aux: None,
            aux_w: 0.0,
            aux_ce: 0.0,
            aux_acc: 0.0,
            heads: Vec::new(),
            dir: None,
            abar: Vec::new(),
            stm: None,
            coh: None,
        };
        net.set_circular(circular);
        if nk_hdr > 0 {
            net.set_film(nk_hdr);
            if it.next() != Some("FILM") {
                return Err(format!("{}: 缺 FILM 段", path.display()));
            }
            let _n = take(&mut it, path, "FILM 数量")? as usize;
            for _ in 0..nl {
                let idx = take(&mut it, path, "Film 层号")? as usize;
                let k = take(&mut it, path, "Film nk")? as usize;
                let c = take(&mut it, path, "Film c")? as usize;
                if k == 0 || c == 0 {
                    continue;
                }
                if let Some(f) = net.films[idx].as_mut() {
                    for v in f.g.iter_mut() {
                        *v = take(&mut it, path, "Film g")?;
                    }
                    for v in f.b.iter_mut() {
                        *v = take(&mut it, path, "Film b")?;
                    }
                }
            }
        }
        if nheads > 0 {
            if it.next() != Some("HEADS") {
                return Err(format!("{}: 缺 HEADS 段", path.display()));
            }
            let _n = take(&mut it, path, "HEADS 数量")? as usize;
            net.heads = Vec::with_capacity(nheads);
            for _ in 0..nheads {
                let on = take(&mut it, path, "head on")? != 0.0;
                let cin = take(&mut it, path, "head cin")? as usize;
                let cout = take(&mut it, path, "head cout")? as usize;
                if !on {
                    net.heads.push(None);
                    continue;
                }
                let mut c = Conv::new(cin, cout, 1);
                for v in c.w.iter_mut() {
                    *v = take(&mut it, path, "head w")?;
                }
                for v in c.b.iter_mut() {
                    *v = take(&mut it, path, "head b")?;
                }
                for v in c.scale.iter_mut() {
                    *v = take(&mut it, path, "head scale")?;
                }
                net.heads.push(Some(c));
            }
        }
        net.set_dilation(dil);
        Ok(net)
    }
}
