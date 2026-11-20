use super::cap::MAX_CH;
use super::conv::Conv;
use super::net::Net;

mod save;
mod sections;

/// 外部文本声明的尺寸上限。`.tpc` 是外部文件：按它写的层数/通道先分配
/// （`Vec::with_capacity(n)`、`Conv::new(cin, cout, 1)`）能把进程直接打爆，
/// 所以每个尺寸都是先查再分，不在分配之后才校验。
const MAX_LAYERS: usize = 1024;
/// 条件通道数上限（`cond = 0` 表示无条件）。`Net::input` 会为每个 `cond` 追加一张 `w×h` 常数图。
const MAX_COND: usize = 64;
/// FiLM 类别数上限（`nk = 0` 表示无 FiLM）。`Film` 的 g/b 各是 `(nk + 1) × cout`。
const MAX_CLASSES: usize = 1024;
/// 膨胀率上限。`Conv` 里 `dil` 要 `as i32` 再参与索引，2^31 之上会直接变负。
const MAX_DIL: usize = 64;

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
        /// 允许 0（= 关闭）的头部字段：`cond` / `nk` 都是「0 表示不用这个特性」。
        fn capped(
            n: usize,
            path: &std::path::Path,
            what: &str,
            max: usize,
        ) -> Result<usize, String> {
            if n <= max {
                Ok(n)
            } else {
                Err(format!(
                    "{}: {what} 不能超过 {max}（收到 {n}）",
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
        let cond = capped(
            take(&mut it, path, "cond")? as usize,
            path,
            "cond",
            MAX_COND,
        )?;
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
            capped(take(&mut it, path, "nk")? as usize, path, "nk", MAX_CLASSES)?
        } else {
            0
        };
        let nheads = if matches!(head, Some("TPC6") | Some("TPC7")) {
            take(&mut it, path, "nheads")? as usize
        } else {
            0
        };
        let dil = if head == Some("TPC7") {
            capped(
                take(&mut it, path, "dilation")? as usize,
                path,
                "dilation",
                MAX_DIL,
            )?
            .max(1)
        } else {
            1
        };
        let mut layers = Vec::with_capacity(nl);
        for _ in 0..nl {
            let cin = bounded(take(&mut it, path, "cin")? as usize, path, "cin", MAX_CH)?;
            let cout = bounded(take(&mut it, path, "cout")? as usize, path, "cout", MAX_CH)?;
            let mut l = Conv::new(cin, cout, 1)?;
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
        let bns = sections::load_bns(&mut it, path, &layers, nl)?;
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
            sections::load_films(&mut it, path, &mut net, nl, nk_hdr)?;
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
                let mut c = Conv::new(cin, cout, 1)?;
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
