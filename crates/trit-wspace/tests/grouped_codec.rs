//! WSP1G 往返、校验和，以及**文本格式下的诚实压缩率**。

use trit_wspace::grouped::GroupedLayer;
use trit_wspace::grouped_codec::{checksum, from_wspg, to_wspg};
use trit_wspace::trit::Trit;

fn small() -> GroupedLayer {
    GroupedLayer::from_trits(
        2,
        3,
        &[1, 0, -1, 1, -1, 0],
        &[0, 1, 0, 1, 0, 1],
        &[0.5, 2.0],
        &[0.25, -0.75],
    )
    .expect("layer")
}

#[test]
fn round_trip_preserves_everything() {
    let layer = small();
    let text = to_wspg(&layer);
    let back = from_wspg(&text).expect("parse");
    assert_eq!(back.rows(), layer.rows());
    assert_eq!(back.cols(), layer.cols());
    assert_eq!(back.group_count(), layer.group_count());
    for row in 0..layer.rows() {
        for col in 0..layer.cols() {
            assert_eq!(back.trit(row, col), layer.trit(row, col));
            assert_eq!(back.group_of(row, col), layer.group_of(row, col));
        }
    }
    for group in 0..layer.group_count() {
        assert_eq!(back.scale(group), layer.scale(group));
    }
    // 再序列化必须逐字节相同（规范形式）
    assert_eq!(to_wspg(&back), text);
}

#[test]
fn checksum_detects_single_trit_change() {
    let layer = small();
    let text = to_wspg(&layer);
    let tampered = text.replacen("row +-", "row ++", 1);
    assert_ne!(tampered, text, "测试本身要改到内容");
    assert!(
        from_wspg(&tampered).is_err(),
        "改一个 trit 必须让校验和失配"
    );
}

#[test]
fn checksum_is_stable() {
    let layer = small();
    assert_eq!(checksum(&layer), checksum(&small()), "同内容校验和必须一致");
}

#[test]
fn text_formats_are_honestly_measured() {
    // 64×64、group=256：真实字节数对比（不是抽象位宽）。
    let rows = 64usize;
    let cols = 64usize;
    let count = rows * cols;
    let weights: Vec<f32> = (0..count)
        .map(|index| ((index % 97) as f32 - 48.0) / 1000.0)
        .collect();
    let bias = vec![0.0f32; rows];

    let layer = GroupedLayer::quantize(rows, cols, &weights, &bias, 256).expect("layer");
    let text = to_wspg(&layer);
    let text_bytes = text.len();
    let f32_bytes = count * 4;

    // 1) 文本一定比 f32 小
    assert!(
        text_bytes < f32_bytes,
        "wsp1g {text_bytes} 应小于 f32 {f32_bytes}"
    );

    // 2) 但**远达不到**"每 trit 2 bit"的 16×：文本里一个 trit 就是一个 ASCII 字节，
    //    再加上组号，实际压缩率在 2× 上下。
    // 实测只有 1.1~1.6×：trit 占 1 字节、组号在文本里每元素还要 2~3 字符，
    // 所以"三值 16×"和"分组 2.4×"都达不到——文本格式的固有代价。
    let ratio = f32_bytes as f64 / text_bytes as f64;
    assert!(
        ratio > 1.0 && ratio < 2.0,
        "文本格式实测压缩率应在 1~2×（实测 {ratio:.2}×）"
    );

    // 3) 组号开销必须体现出来：trits 只占 count 字节，其余都是头部/scale/组号。
    assert!(
        text_bytes > count,
        "组号等开销应让总字节数超过 count（{text_bytes} vs {count}）"
    );
    println!("f32 {f32_bytes} B | wsp1g {text_bytes} B | 压缩 {ratio:.2}× | trit 本体 {count} B");
}

#[test]
fn per_tensor_three_valued_text_is_only_4x() {
    // 每个 trit 一个 ASCII 字节 ⇒ 三值文本理论上限就是 4×（32bit → 8bit），
    // 不是常说的 16×。这里用 group=整个张量来逼近该上限。
    let rows = 8usize;
    let cols = 8usize;
    let count = rows * cols;
    let weights: Vec<f32> = (0..count)
        .map(|index| ((index % 13) as f32) / 10.0 + 0.01)
        .collect();
    let layer = GroupedLayer::quantize(rows, cols, &weights, &vec![0.0; rows], count).expect("l");
    let text = to_wspg(&layer);
    let ratio = (count * 4) as f64 / text.len() as f64;
    assert!(ratio < 4.2, "三值文本不可能超过 ~4×（实测 {ratio:.2}×）");
    println!("per-tensor 三值文本：{ratio:.2}×（理论上限 4×）");
}

#[test]
fn rejects_wrong_magic() {
    assert!(from_wspg("wsp1\nrows 1\n").is_err(), "魔数不对应报错");
    assert!(from_wspg("").is_err(), "空文本应报错");
}

#[test]
fn rejects_bad_trit_char() {
    let text = "wsp1g\nrows 1\ncols 2\ngroup 2\nscales 1\nbias 0\nrow -x\ngroups 0 0\nend\n";
    assert!(from_wspg(text).is_err(), "非法 trit 字符应报错");
}

#[test]
fn decoded_trits_are_ternary() {
    let back = from_wspg(&to_wspg(&small())).expect("parse");
    for row in 0..back.rows() {
        for col in 0..back.cols() {
            let trit = back.trit(row, col).expect("trit");
            assert!(Trit::ALL.contains(&trit));
        }
    }
}
