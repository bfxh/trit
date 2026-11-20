//! `api` 的测试（从 `api.rs` 下沉）：`res` 是外部数字，解码侧的边界钉在这里。
use super::*;

#[test]
fn decode_respects_manifest_and_valid() {
    let m = PartList::parse(
            r#"{"k":2,"parts":[{"id":1,"digits":[0,0]},{"id":2,"digits":[0,1]},{"id":4,"digits":[1,0]}]}"#,
        )
        .unwrap();
    // res=2 ⇒ 4 个像素；只给像素 0 有效（且指向 id 4），像素 3 的 valid 置负
    let (res, ch) = (2u32, 7usize);
    let hw = crate::dims::pixels(res as usize, res as usize);
    let mut lg = vec![0f32; hw * ch];
    lg[1] = 5.0; // plane0 → digit 1
    lg[3] = 5.0; // plane1 → digit 0
    lg[6] = 5.0; // valid
    let p3 = 3 * ch;
    lg[p3 + 6] = -5.0;
    let ids = decode_ids(&lg, res, &m);
    assert_eq!(ids[0], 4);
    assert_eq!(ids[3], 0);
}

/// `res` 是外部给的分辨率，而 `decode_ids` 不返回 Result：0 会让取模除零。
#[test]
#[should_panic(expected = "分辨率不能为 0")]
fn decode_rejects_zero_res() {
    let m = PartList::parse(r#"{"k":1,"parts":[{"id":1,"digits":[0]}]}"#).unwrap();
    let _ = decode_ids(&[], 0, &m);
}

/// 非网络的解码入口只要求 res 非 0：1 像素的网格是合法的。
#[test]
fn decode_accepts_single_pixel_grid() {
    let m = PartList::parse(r#"{"k":1,"parts":[{"id":1,"digits":[0]},{"id":2,"digits":[1]}]}"#)
        .unwrap();
    // res=1 ⇒ 1 个像素，ch=4（1+3 位）
    let lg = [0.0f32, 0.0, 0.0, 5.0];
    let ids = decode_ids(&lg, 1, &m);
    assert_eq!(ids.len(), 1);
    assert_eq!(ids[0], 1, "plane0 取 0 位 → digit 0 → id 1");
}
