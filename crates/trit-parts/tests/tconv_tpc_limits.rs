//! `tconv::Net::load` 的尺寸契约：`.tpc` 里的层数与通道数是外部文本，
//! 按它们先分配会直接把进程打爆，所以必须在 `Vec::with_capacity` / `Conv::new`
//! 之前就拒掉。

use trit_parts::tconv::Net;

fn tmp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("trit-tpc-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn absurd_layer_count_is_rejected_before_allocating() {
    let d = tmp_dir("layers");
    let p = d.join("net.tpc");
    // 头字段：TPC7 <层数> <cond> <linear> <circular> <highpass> <nk> <nheads> <dilation>
    std::fs::write(&p, "TPC7 999999999 0 0 0 0 0 0 1\n").unwrap();
    let err = match Net::load(&p) {
        Ok(_) => panic!("层数 999999999 必须在分配前被拒"),
        Err(e) => e,
    };
    assert!(err.contains("层数"), "{err}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn absurd_channel_is_rejected_before_allocating() {
    let d = tmp_dir("channels");
    let p = d.join("net.tpc");
    std::fs::write(&p, "TPC7 1 0 0 0 0 0 0 1\n100000 1\n").unwrap();
    let err = match Net::load(&p) {
        Ok(_) => panic!("cin 100000 必须在分配前被拒"),
        Err(e) => e,
    };
    assert!(err.contains("cin"), "{err}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn in_range_net_roundtrips() {
    let d = tmp_dir("roundtrip");
    let p = d.join("net.tpc");
    let net = Net::new(2, &[4, 3], 7);
    net.save(&p).unwrap();
    let back = Net::load(&p).expect("正常尺寸必须读得回来");
    assert_eq!(back.layers.len(), net.layers.len());
    assert_eq!(back.layers[0].cin, net.layers[0].cin);
    assert_eq!(back.layers[0].cout, net.layers[0].cout);
    let _ = std::fs::remove_dir_all(&d);
}
