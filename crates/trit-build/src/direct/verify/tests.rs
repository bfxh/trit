//! `--verify-c` 驱动器源码的签名契约。

use super::driver_source;

#[test]
fn driver_source_declares_int8_return() {
    let driver = driver_source(2);
    assert!(driver.contains("int8_t f(int8_t *v, int n);"), "{driver}");
    assert!(
        driver.contains("int main(int argc, char **argv)"),
        "{driver}"
    );
}
