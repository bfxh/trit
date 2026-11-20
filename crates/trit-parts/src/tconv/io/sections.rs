//! `.tpc` 的 BN / FILM 两段（从 `tconv/io.rs` 下沉）。
//!
//! 两段的共同点：**文件里的数字只用来对账，不用来分配**。BN 的通道数一律取自
//! `layers[idx].cout`；FILM 的 `(nk, c)` 必须与 `set_film(nk_hdr)` + `layers[idx].cout`
//! 造出来的 `Film` 逐个对上 —— 否则读入个数与实际长度不一致，后面的 `HEADS` 段会整体偏位。

use std::path::Path;

use super::super::bn::Bn;
use super::super::conv::Conv;
use super::super::net::Net;

fn take<'a>(
    it: &mut impl Iterator<Item = &'a str>,
    path: &Path,
    what: &str,
) -> Result<f32, String> {
    it.next()
        .ok_or_else(|| format!("{}: 缺 {what}", path.display()))?
        .parse::<f32>()
        .map_err(|e| format!("{}: {what} 解析失败 {e}", path.display()))
}

/// 读 BN 段（`BN <数量>` 之后 `nl` 行 `<层号> <开关>`，启用时再跟 γ/β 两行）。
pub(super) fn load_bns<'a>(
    it: &mut impl Iterator<Item = &'a str>,
    path: &Path,
    layers: &[Conv],
    nl: usize,
) -> Result<Vec<Option<Bn>>, String> {
    let mut bns: Vec<Option<Bn>> = (0..nl).map(|_| None).collect();
    if it.next() != Some("BN") {
        return Err(format!("{}: 缺 BN 段", path.display()));
    }
    let _n = take(it, path, "BN 数量")? as usize;
    for _ in 0..nl {
        let idx = take(it, path, "BN 层号")? as usize;
        let on = take(it, path, "BN 开关")? != 0.0;
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
            *v = take(it, path, "gamma")?;
        }
        for v in b.beta.iter_mut() {
            *v = take(it, path, "beta")?;
        }
        bns[idx] = Some(b);
    }
    Ok(bns)
}

/// 读 FILM 段（`FILM <数量>` 之后 `nl` 行 `<层号> <nk> <c>`，启用时再跟 g/b 两行）。
///
/// 读入个数由 `Film` 的实际长度决定，文件里的 `(nk, c)` 只是声明。声明不符时不 panic、
/// 也不会报出真正的原因 —— 多读或少读的数字会把 `HEADS` 段挤偏，最后报成「缺 HEADS 段」。
/// 所以在读之前先对账。
pub(super) fn load_films<'a>(
    it: &mut impl Iterator<Item = &'a str>,
    path: &Path,
    net: &mut Net,
    nl: usize,
    nk_hdr: usize,
) -> Result<(), String> {
    let _n = take(it, path, "FILM 数量")? as usize;
    for _ in 0..nl {
        let idx = take(it, path, "Film 层号")? as usize;
        let k = take(it, path, "Film nk")? as usize;
        let c = take(it, path, "Film c")? as usize;
        // `save` 对不启用 FiLM 的层写的是 `0 0`；只认这一种，`0 32` 这种半个零一样是错声明。
        if k == 0 && c == 0 {
            continue;
        }
        if idx >= nl {
            return Err(format!(
                "{}: FILM 层号 {idx} 越界（层数 {nl}）",
                path.display()
            ));
        }
        let cout = net.layers[idx].cout;
        if k != nk_hdr || c != cout {
            return Err(format!(
                "{}: FILM 声明与网络不符（层号 {idx}：nk {k} 应为 {nk_hdr}，c {c} 应为 {cout}）",
                path.display()
            ));
        }
        match net.films[idx].as_mut() {
            Some(f) => {
                for v in f.g.iter_mut() {
                    *v = take(it, path, "Film g")?;
                }
                for v in f.b.iter_mut() {
                    *v = take(it, path, "Film b")?;
                }
            }
            None => {
                return Err(format!(
                    "{}: FILM 层号 {idx} 是输出层，`set_film` 不给它建 FiLM",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}
