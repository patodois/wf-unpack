#![no_main]
use libfuzzer_sys::fuzz_target;
use wf_unpack::level::{RawLevel, StaticLevel};

fuzz_target!(|data: &[u8]| {
    let [head, body] = wf_unpack_fuzz::split::<2>(data);
    if let Ok(l) = RawLevel::parse(head, body) {
        for o in l.objects.iter().take(64) {
            let _ = o.attrs();
        }
    }
    let _ = StaticLevel::parse(head, body);
});
