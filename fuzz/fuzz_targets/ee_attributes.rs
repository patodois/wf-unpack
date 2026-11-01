#![no_main]
use libfuzzer_sys::fuzz_target;
use wf_unpack::ee::{parse_value, Attributes};

fuzz_target!(|data: &[u8]| {
    let text = wf_unpack_fuzz::text(data);
    if let Ok(a) = Attributes::parse(&text) {
        for (k, v) in &a.0 {
            let _ = (v.text(), v.items(), v.floats(), v.float(), v.get(k));
            let _ = (a.text(k), a.float(k), a.floats(k), a.flag(k));
        }
    }
    let _ = parse_value(&text);
});
