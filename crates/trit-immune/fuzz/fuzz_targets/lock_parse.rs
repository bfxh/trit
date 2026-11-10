#![no_main]

use libfuzzer_sys::fuzz_target;
use trit_immune::parse_lock;

fuzz_target!(|data: &[u8]| {
    let _ = parse_lock(&String::from_utf8_lossy(data));
});
