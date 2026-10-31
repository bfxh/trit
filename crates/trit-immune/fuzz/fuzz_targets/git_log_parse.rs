#![no_main]

use libfuzzer_sys::fuzz_target;
use trit_immune::parse_git_log;

fuzz_target!(|data: &[u8]| {
    let _ = parse_git_log(&String::from_utf8_lossy(data));
});
