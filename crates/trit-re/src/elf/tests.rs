//! elf 的单元测试（从 src/elf/mod.rs 拆出，控制单文件行数预算）。

use super::{ParseError, parse};

#[test]
fn rejects_non_elf() {
    assert_eq!(parse(b"not elf").unwrap_err(), ParseError("不是 ELF 魔数"));
}

#[test]
fn rejects_big_endian_explicitly() {
    let mut bytes = vec![0u8; 64];
    bytes[..4].copy_from_slice(b"\x7fELF");
    bytes[4] = 2;
    bytes[5] = 2;
    assert_eq!(parse(&bytes).unwrap_err(), ParseError("大端 ELF 暂不支持"));
}

#[test]
fn truncated_header_is_an_error_not_a_panic() {
    let mut bytes = vec![0u8; 20];
    bytes[..4].copy_from_slice(b"\x7fELF");
    bytes[4] = 2;
    bytes[5] = 1;
    assert!(parse(&bytes).is_err());
}
