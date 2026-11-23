//! TPM1 部署包：读侧必须把 `q` 长度与声明维度对账（issue #626）。
//!
//! 修前 `TpmModel::load` 只按头里的 `nw` 切 q 字节，从不校验
//! `nw == kk²·cin·cout`；`nw` 偏长 / 偏短都静默接受，`from_tpm` 再按
//! `q.len()` 建 latent，形状谎报一路传到下游。
//!
//! 回归三件套：
//! * 谎报长度（偏长 / 偏短）被拒，错误串带 `cin/cout/kk/expected/nw`；
//! * 合法回环（含 `kk=2` 卷积层，`nw` 恰好等于期望长度）仍 `Ok`；
//! * 边界：`nw == expected` 放行，`expected ± 1` 拒绝。

use trit_parts::tpm::{TpmLayer, TpmModel};

fn tmp_file(tag: &str, bytes: &[u8]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("trit-tpm-shape-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("model.tpm");
    std::fs::write(&path, bytes).unwrap();
    path
}

/// 手工拼一层 TPM1 字节流；`q` 字节数必须与 `nw` 一致（截断保护另由读侧负责）。
fn layer_bytes(cin: u32, cout: u32, kk: u32, nw: u32, q: &[i8]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(b"TPM1\0\0\0\0");
    b.extend_from_slice(&1u32.to_le_bytes()); // version
    b.extend_from_slice(&1u32.to_le_bytes()); // nlayers
    b.extend_from_slice(&cin.to_le_bytes());
    b.extend_from_slice(&cout.to_le_bytes());
    b.extend_from_slice(&kk.to_le_bytes());
    b.extend_from_slice(&1u32.to_le_bytes()); // relu
    b.extend_from_slice(&0u32.to_le_bytes()); // convt
    b.extend_from_slice(&nw.to_le_bytes());
    let raw_q: Vec<u8> = q.iter().map(|&v| v as u8).collect();
    b.extend_from_slice(&raw_q);
    for _ in 0..cout {
        b.extend_from_slice(&0.5f32.to_le_bytes()); // scale
    }
    for _ in 0..cout {
        b.extend_from_slice(&1.0f32.to_le_bytes()); // gamma
    }
    for _ in 0..cout {
        b.extend_from_slice(&0.0f32.to_le_bytes()); // beta
    }
    for _ in 0..cout {
        b.extend_from_slice(&0.1f32.to_le_bytes()); // bias
    }
    b
}

fn expect_err(bytes: &[u8], what: &str) -> String {
    let path = tmp_file(what, bytes);
    match TpmModel::load(&path) {
        Ok(_) => panic!("{what} 必须被拒"),
        Err(e) => e,
    }
}

#[test]
fn rejects_q_long() {
    // cin=2 cout=2 kk=1 ⇒ expected=4；nw=8 偏长。
    let q = vec![1i8, 0, -1, 1, 0, 1, -1, 0];
    let err = expect_err(&layer_bytes(2, 2, 1, 8, &q), "long");
    for needle in ["cin=2", "cout=2", "kk=1", "expected=4", "nw=8"] {
        assert!(err.contains(needle), "错误串缺 `{needle}`：{err}");
    }
}

#[test]
fn rejects_q_short() {
    // 同一形状，nw=2 偏短。
    let q = vec![1i8, 0];
    let err = expect_err(&layer_bytes(2, 2, 1, 2, &q), "short");
    assert!(err.contains("expected=4"), "{err}");
    assert!(err.contains("nw=2"), "{err}");
}

#[test]
fn rejects_conv_shape_mismatch() {
    // kk=3 卷积：expected = 3²·3·32 = 864；谎报 nw=100。
    let q = vec![0i8; 100];
    let err = expect_err(&layer_bytes(3, 32, 3, 100, &q), "conv");
    assert!(err.contains("expected=864"), "{err}");
    assert!(err.contains("nw=100"), "{err}");
}

#[test]
fn accepts_exact_expected_length() {
    // kk=2 卷积：expected = 2²·2·3 = 24；nw 恰好等于期望长度必须放行。
    let q = (0..24).map(|i| (i % 3) as i8 - 1).collect::<Vec<_>>();
    let path = tmp_file("exact", &layer_bytes(2, 3, 2, 24, &q));
    let model = TpmModel::load(&path).expect("恰好等长应放行");
    assert_eq!(model.layers[0].q.len(), 24);
    assert_eq!(model.layers[0].effective().len(), 24);
}

#[test]
fn legal_roundtrip_still_loads() {
    let model = TpmModel {
        layers: vec![TpmLayer {
            q: vec![1, 0, -1, 1],
            scale: vec![0.5, 0.25],
            gamma: vec![1.0, 1.0],
            beta: vec![0.0, 0.0],
            bias: vec![0.1, -0.2],
            cin: 2,
            cout: 2,
            kk: 1,
            relu: true,
            convt: false,
        }],
    };
    let dir = std::env::temp_dir().join(format!("trit-tpm-rt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("roundtrip.tpm");
    model.save(&path).unwrap();
    let back = TpmModel::load(&path).expect("合法回环必须仍能读");
    assert_eq!(back.layers[0].q, model.layers[0].q);
    assert_eq!(back.layers[0].effective(), model.layers[0].effective());
}
