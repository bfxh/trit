use super::*;

mod activation;

fn write_test_ply(rest_props: usize) -> Vec<u8> {
    let mut head = String::from(
        "ply\nformat binary_little_endian 1.0\nelement vertex 2\n\
         property float x\nproperty float y\nproperty float z\n\
         property float f_dc_0\nproperty float f_dc_1\nproperty float f_dc_2\n",
    );
    for k in 0..rest_props {
        head.push_str(&format!("property float f_rest_{k}\n"));
    }
    head.push_str(
        "property float opacity\nproperty float scale_0\nproperty float scale_1\n\
         property float scale_2\nproperty float rot_0\nproperty float rot_1\n\
         property float rot_2\nproperty float rot_3\nend_header\n",
    );
    let mut out = head.into_bytes();
    for v in 0..2u32 {
        // 全部 14(+rest) 列都要写数据
        let vals: Vec<f32> = (0..(14 + rest_props))
            .map(|k| (v as f32) + 0.25 * (k as f32))
            .collect();
        for f in vals {
            out.extend_from_slice(&f.to_le_bytes());
        }
    }
    out
}

#[test]
fn parses_standard_layout_by_property_names() {
    let bytes = write_test_ply(0);
    let g = parse_ply(&bytes).unwrap();
    assert_eq!(g.len(), 2);
    // 数据 = v + 0.25·列序：v=0 第 7 列 (opacity) = 1.5；v=1 第 10 列 (scale_2) = 3.25
    assert!((g[1].position[0] - 1.0).abs() < 1e-6);
    assert!(
        (g[0].opacity_logit - 1.5).abs() < 1e-6,
        "opacity={}",
        g[0].opacity_logit
    );
    assert!((g[1].scale_log[2] - 3.25).abs() < 1e-6);
    assert!(g[0].f_rest.is_empty());
}

#[test]
fn parses_sh_rest_columns() {
    let bytes = write_test_ply(45);
    let g = parse_ply(&bytes).unwrap();
    assert_eq!(g[0].f_rest.len(), 45);
}

#[test]
fn parses_headerless_gaussforge_variant() {
    // 字节级抹掉 end_header 行，模拟 gaussforge 回转 PLY（头后直接二进制）
    let mut bytes = write_test_ply(0);
    let needle = b"end_header\n";
    let pos = bytes
        .windows(needle.len())
        .position(|w| w == needle)
        .expect("end_header 应存在");
    bytes.drain(pos..pos + needle.len());
    let g = parse_ply(&bytes).unwrap();
    assert_eq!(g.len(), 2);
}

#[test]
fn missing_required_property_fails() {
    // 字节级删掉 opacity 属性行（头是 ASCII，按行删安全）
    let mut bytes = write_test_ply(0);
    let needle = b"property float opacity\n";
    let pos = bytes
        .windows(needle.len())
        .position(|w| w == needle)
        .expect("opacity 属性行应存在");
    bytes.drain(pos..pos + needle.len());
    let err = parse_ply(&bytes).unwrap_err();
    assert!(matches!(err, SplatError::MissingProperty(_)));
}
