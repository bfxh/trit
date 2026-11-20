//! `Network::random` 的规模闸：分配 `size²` 之前先判 `MAX_NEURONS`。
//!
//! `validate()` 也查同一个上限，但它只在 `simulate`/`run` 里调用——那时
//! `weights` 已经按 `size²` 建好了，闸来晚了。

use trit_machines::snn::{MAX_NEURONS, Network, NetworkError};

#[test]
fn random_above_the_cap_is_rejected_before_allocating() {
    assert!(Network::random(MAX_NEURONS, 1).is_ok(), "上限本身必须合法");
    assert!(matches!(
        Network::random(MAX_NEURONS + 1, 1),
        Err(NetworkError::Limit(_))
    ));
    assert!(Network::random(usize::MAX, 1).is_err());
}
