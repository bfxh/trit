//! 三值化：把 f32 权重压成 `{−scale, 0, +scale}`。

/// 三值化模式。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Quant {
    None,
    /// 整个矩阵一个 scale。
    Sign,
    /// 每行一个 scale。
    SignRow,
}

impl Quant {
    /// 从环境变量 `WSP_QUANT` 解析（缺省 `none`）。
    pub fn from_env() -> Self {
        match std::env::var("WSP_QUANT").unwrap_or_default().as_str() {
            "" | "none" => Quant::None,
            "sign" => Quant::Sign,
            "sign_row" => Quant::SignRow,
            other => {
                eprintln!("WSP_QUANT 只支持 none / sign / sign_row，收到 {other}");
                std::process::exit(2);
            }
        }
    }
}

/// 按行主序 `rows × cols` 量化。
pub fn quantize_matrix(v: &mut [f32], rows: usize, cols: usize, mode: Quant) {
    match mode {
        Quant::None => {}
        Quant::Sign => {
            let nz = v.iter().filter(|x| **x != 0.0).count().max(1);
            let scale = v.iter().map(|x| x.abs()).sum::<f32>() / nz as f32;
            for x in v.iter_mut() {
                let q = if *x == 0.0 { 0.0 } else { x.signum() };
                *x = scale * q;
            }
        }
        Quant::SignRow => {
            for r in 0..rows {
                let row = &mut v[r * cols..(r + 1) * cols];
                let nz = row.iter().filter(|x| **x != 0.0).count().max(1);
                let scale = row.iter().map(|x| x.abs()).sum::<f32>() / nz as f32;
                for x in row.iter_mut() {
                    let q = if *x == 0.0 { 0.0 } else { x.signum() };
                    *x = scale * q;
                }
            }
        }
    }
}
