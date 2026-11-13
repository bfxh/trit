//! 把一条 k 曲线组装成 JSON：一致率 + 七列诊断 + WSP 体积。

use serde_json::json;
use trit_wspace::Net;
use trit_wspace::agree::measure;

pub fn curve(k: usize, rate: f64, q: &[f64; 8], full: &Net, small: &Net) -> serde_json::Value {
    let full_size = measure(full).expect("wsp2");
    let size = measure(small).expect("wsp2");
    json!({
        "k": k, "rate": rate,
        "f32": q[0], "w_trit": q[1], "x_trit": q[2], "trit_both": q[3],
        "trit_scale": q[4], "trit_readout": q[5], "trit_stage": q[6], "trit_vs_f32": q[7],
        "trits": size.trits, "full_trits": full_size.trits,
        "wsp1": size.wsp1, "full_wsp1": full_size.wsp1,
        "wsp2": size.wsp2, "full_wsp2": full_size.wsp2,
        "wsp2_ratio": full_size.wsp2 as f64 / size.wsp2 as f64,
    })
}
