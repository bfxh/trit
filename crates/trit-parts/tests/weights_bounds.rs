//! 权重索引：`weights.json` 的声明数 / 形状 / 偏移全部是外部输入。
//!
//! 三处都是「先按声明分配、再用溢出的算术去校验」：
//!
//! * `Tensors::load` 里 `off * 4` 与 `(off + n) * 4` 是裸乘，绕回后可以落进
//!   真实字节范围，于是 `raw.get(…)` 通过、`Vec::with_capacity(n)` 按声明数分配；
//! * `conv_w` 的 `kk * kk * cin * cout` 与 `convt_w` 的 `4 * cin * cout` 同理，
//!   绕成 0 时缓冲是空的、后面的 `w[…]` 才越界；不绕回而是真大时是两份按声明分配；
//! * `conv_w` 的 BN 四个张量与 `convt_w` 的 bias 从不与 `cout` 比对。
//!
//! 修法统一成「先算再比」：`checked_mul` 链算 `n`，再拿它和 `w.len()` 对账 ——
//! `w.len()` 由 `weights.bin` 的真实字节数决定，任何 json 声明都绕不过它。

use std::collections::HashMap;
use trit_parts::weights::{Tensors, conv_w, convt_w};

fn tmp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("trit-wts-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// `unwrap_err` 会要求 `Tensors` / `ConvW` 实现 `Debug`；这里只取错误串。
fn err_of<T>(r: Result<T, String>, what: &str) -> String {
    match r {
        Ok(_) => panic!("{what} 必须被拒"),
        Err(e) => e,
    }
}

fn tmap(items: &[(&str, Vec<f32>, Vec<usize>)]) -> Tensors {
    let mut data = HashMap::new();
    for (n, v, s) in items {
        data.insert((*n).to_string(), (v.clone(), s.clone()));
    }
    Tensors { data }
}

/// `off * 4` / `(off + n) * 4` 绕回后正好落进真实字节范围 —— 长度校验通过，
/// 于是 `Vec::with_capacity(n)` 拿 `2^62` 去分配。
#[test]
fn absurd_byte_offset_is_rejected_before_capacity() {
    let d = tmp_dir("woff");
    std::fs::write(d.join("weights.bin"), vec![0u8; 1000]).unwrap();
    let off = 1usize << 62;
    let n = (1usize << 62) + 250;
    std::fs::write(
        d.join("weights.json"),
        format!("{{ \"name\": \"w\", \"off\": {off}, \"n\": {n}, \"shape\": [250] }}"),
    )
    .unwrap();
    let r = Tensors::load(&d);
    let _ = std::fs::remove_dir_all(&d);
    let e = err_of(r, "off 2^62");
    assert!(e.contains("溢出"), "{e}");
}

/// `kk * kk * cin * cout` 在 64 位绕成 0：缓冲长度 0，`w[…]` 越界。
#[test]
fn shape_product_overflow_is_rejected_before_allocating() {
    let big = 1usize << 32;
    let t = tmap(&[
        ("w.weight", vec![0.0; 1], vec![1, big, big, big]),
        ("w.bias", vec![0.0; 1], vec![1]),
    ]);
    let e = err_of(conv_w(&t, "w.weight", None, false), "shape 溢出");
    assert!(e.contains("相乘溢出"), "{e}");
}

/// 声明的形状推不出真实张量长度：修前是 `vec![0f32; 16]` 之后再 `w[1]` 越界。
#[test]
fn shape_must_match_the_real_tensor_length() {
    let t = tmap(&[
        ("w.weight", vec![0.0; 1], vec![4, 4, 1, 1]),
        ("w.bias", vec![0.0; 4], vec![4]),
    ]);
    let e = err_of(conv_w(&t, "w.weight", None, false), "shape 溢出");
    assert!(e.contains("张量长度") && e.contains("不符"), "{e}");
}

/// BN 的四个张量都按 `cout` 取用，短一个就在 `g[c]` 越界。
#[test]
fn bn_tensors_must_match_the_channel_count() {
    let t = tmap(&[
        ("w.weight", vec![0.0; 16], vec![4, 4, 1, 1]),
        ("w.bias", vec![0.0; 4], vec![4]),
        ("bn.weight", vec![1.0; 2], vec![2]),
        ("bn.bias", vec![0.0; 4], vec![4]),
        ("bn.running_mean", vec![0.0; 4], vec![4]),
        ("bn.running_var", vec![1.0; 4], vec![4]),
    ]);
    let e = err_of(conv_w(&t, "w.weight", Some("bn"), false), "bn 长度");
    assert!(e.contains("bn.weight") && e.contains("不符"), "{e}");
}

/// 转置卷积的 bias 长度不校验：修前不崩，静默返回一个比 `cout` 短的 bias。
#[test]
fn convt_bias_must_match_the_channel_count() {
    let t = tmap(&[
        ("w.weight", vec![0.0; 32], vec![2, 4, 2, 2]),
        ("w.bias", vec![0.0; 1], vec![1]),
    ]);
    let e = err_of(convt_w(&t, "w.weight"), "convt bias 长度");
    assert!(e.contains("张量长度") && e.contains("不符"), "{e}");
}
