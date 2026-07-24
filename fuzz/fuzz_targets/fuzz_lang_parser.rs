#![no_main]

use arc_lang::ast::{LanguagePlugin, RustPlugin};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let plugin = RustPlugin::new();
    let source = String::from_utf8_lossy(data);
    let _ = plugin.parse(&source);
});
