#![no_main]

use libfuzzer_sys::fuzz_target;
use trit_immune::parse_stamped_line;

fuzz_target!(|data: &[u8]| {
    // 单行日志：坏行必须显式报错，永不 panic
    let _ = parse_stamped_line(&String::from_utf8_lossy(data));
});
