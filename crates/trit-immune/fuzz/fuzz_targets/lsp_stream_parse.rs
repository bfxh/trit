#![no_main]

use libfuzzer_sys::fuzz_target;
use trit_immune::parse_lsp_stream;

fuzz_target!(|data: &[u8]| {
    let _ = parse_lsp_stream(&String::from_utf8_lossy(data));
});
