//! `.tpc` 头部剩余三个数字（`cond` / `nk` / `dilation`）与 FILM 段的声明自洽性。
//!
//! `#554` 把 `层数` / `cin` / `cout` / `nheads` 收进了 `bounded`，但同一段里的
//! `cond`、`nk`、`dilation` 还是裸读：它们各自决定一处分配或一次 `as i32`，
//! 而 FILM 段写在文件里的 `(nk, c)` 与 `set_film` + `layers[idx].cout`
//! 造出来的个数之间从没有比对过 —— 不符时不会崩，只会读错个数，
//! 把后面的段整体挤偏，最后报成「缺 HEADS 段」。

use trit_parts::tconv::Net;

fn tmp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("trit-tpc-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn load_body(tag: &str, body: &str) -> Result<Net, String> {
    let d = tmp_dir(tag);
    let p = d.join("net.tpc");
    std::fs::write(&p, body).unwrap();
    let r = Net::load(&p);
    let _ = std::fs::remove_dir_all(&d);
    r
}

fn expect_err(r: Result<Net, String>, tag: &str) -> String {
    match r {
        Ok(_) => panic!("{tag} 必须被拒"),
        Err(e) => e,
    }
}

/// `cond` 决定 `Net::input` 追加多少张 `w×h` 常数图；它没有 `bounded`。
#[test]
fn absurd_cond_is_rejected_before_allocating() {
    // TPC7 <层数> <cond> <linear> <circular> <highpass> <nk> <nheads> <dilation>
    let err = expect_err(
        load_body("cond", "TPC7 2 1000000 0 0 0 0 0 1\n"),
        "cond 1000000",
    );
    assert!(err.contains("cond 不能超过"), "{err}");
}

/// `nk` 决定 `Film` 的 g/b 长度 `(nk+1)×cout`，同样没有 `bounded`。
#[test]
fn absurd_class_count_is_rejected_before_film_allocates() {
    let err = expect_err(
        load_body("nk", "TPC7 1 0 0 0 0 999999999 0 1\n"),
        "nk 999999999",
    );
    assert!(err.contains("nk 不能超过"), "{err}");
}

/// `dilation` 在 `Conv` 里要 `as i32` 再参与索引，2^31 之上直接变负。
#[test]
fn absurd_dilation_is_rejected_before_the_i32_cast() {
    let err = expect_err(
        load_body("dil", "TPC7 1 0 0 0 0 0 0 65536\n"),
        "dilation 65536",
    );
    assert!(err.contains("dilation 不能超过"), "{err}");
}

/// FILM 段里的 `c` 与 `layers[idx].cout` 不符：读入个数会与 `Film` 的实际长度对不上，
/// 后面的 `b` 行与 `HEADS` 段整体偏位。
#[test]
fn film_declaration_must_match_the_network() {
    let d = tmp_dir("film");
    let p = d.join("net.tpc");
    let mut net = Net::new(2, &[4, 3], 7);
    net.set_film(2);
    net.save(&p).unwrap();
    // 只把 FILM 段第一行的 c 从 4 改成 8，g/b 仍是 (2+1)×4 = 12 个数。
    let txt = std::fs::read_to_string(&p).unwrap();
    let mut out = String::new();
    let (mut in_film, mut done) = (false, false);
    for line in txt.lines() {
        if line.starts_with("FILM ") {
            in_film = true;
        } else if in_film && !done && line.trim_end().ends_with("2 4") {
            out.push_str(&line.replace("2 4", "2 8"));
            out.push('\n');
            done = true;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    std::fs::write(&p, out).unwrap();

    let err = expect_err(Net::load(&p), "FILM c=8");
    assert!(err.contains("FILM 声明与网络不符"), "报错误导：{err}");
    let _ = std::fs::remove_dir_all(&d);
}
