use super::bn::Bn;
use super::conv::Conv;
use super::net::Net;

mod save;

/// 外部文本声明的尺寸上限。`.tpc` 是外部文件：按它写的层数/通道先分配
/// （`Vec::with_capacity(n)`、`Conv::new(cin, cout, 1)`）能把进程直接打爆，
/// 所以每个尺寸都是先查再分，不在分配之后才校验。
const MAX_LAYERS: usize = 1024;
const MAX_CH: usize = 2048;

impl Net {
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
        fn bounded(
            n: usize,
            path: &std::path::Path,
            what: &str,
            max: usize,
        ) -> Result<usize, String> {
            if (1..=max).contains(&n) {
                Ok(n)
            } else {
                Err(format!(
                    "{}: {what} 必须在 1..={max}（收到 {n}）",
                    path.display()
                ))
            }
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
        let nl = bounded(
            take(&mut it, path, "层数")? as usize,
            path,
            "层数",
            MAX_LAYERS,
        )?;
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
            let cin = bounded(take(&mut it, path, "cin")? as usize, path, "cin", MAX_CH)?;
            let cout = bounded(take(&mut it, path, "cout")? as usize, path, "cout", MAX_CH)?;
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
            if idx >= nl {
                return Err(format!(
                    "{}: BN 层号 {idx} 越界（层数 {nl}）",
                    path.display()
                ));
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
                if k > MAX_LAYERS || c > MAX_CH || idx >= nl {
                    return Err(format!(
                        "{}: Film 条目越界（层号 {idx}/{nl}，nk {k}，c {c}）",
                        path.display()
                    ));
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
        if nheads > MAX_LAYERS {
            return Err(format!(
                "{}: nheads 不能超过 {MAX_LAYERS}（收到 {nheads}）",
                path.display()
            ));
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
                let cin = bounded(cin, path, "head cin", MAX_CH)?;
                let cout = bounded(cout, path, "head cout", MAX_CH)?;
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
