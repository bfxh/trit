//! WSP 编码体积：降维网络相对原网络省多少。
//!
//! 体积口径用 WSP2（每 trit 2 bit 的定长容器），包含 bias 与每层 header；
//! WSP1 文本（每 trit 一字符）只作参考。

use serde_json::json;
use trit_wspace::Net;

/// `(权重+bias 的 trits, WSP1 字节, WSP2 字节)`。
fn measure(net: &Net) -> (usize, usize, usize) {
    let trits: usize = net
        .layers()
        .iter()
        .map(|layer| layer.trits().len() + layer.bias().len())
        .sum();
    let wsp1 = trit_wspace::to_wsp(net).len();
    let wsp2 = trit_wspace::to_wsp2(net).expect("wsp2").len();
    (trits, wsp1, wsp2)
}

/// 一条 k 曲线的完整读数：一致率、七列诊断与体积。
pub fn curve(k: usize, rate: f64, q: &[f64; 8], full: &Net, small: &Net) -> serde_json::Value {
    let (full_trits, full_wsp1, full_wsp2) = measure(full);
    let (trits, wsp1, wsp2) = measure(small);
    json!({
        "k": k, "rate": rate,
        "f32": q[0], "w_trit": q[1], "x_trit": q[2], "trit_both": q[3],
        "trit_scale": q[4], "trit_readout": q[5], "trit_stage": q[6], "trit_vs_f32": q[7],
        "trits": trits, "full_trits": full_trits,
        "wsp1": wsp1, "full_wsp1": full_wsp1,
        "wsp2": wsp2, "full_wsp2": full_wsp2,
        "wsp2_ratio": full_wsp2 as f64 / wsp2 as f64,
    })
}
