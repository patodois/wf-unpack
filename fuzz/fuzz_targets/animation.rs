#![no_main]
use libfuzzer_sys::fuzz_target;
use wf_unpack::animation::{decode, parse_events, AnimHeader};

fuzz_target!(|data: &[u8]| {
    let [head, body] = wf_unpack_fuzz::split::<2>(data);
    if let Ok(h) = AnimHeader::parse(head) {
        let _ = parse_events(&h.common.attributes);
    }
    if let Ok(clips) = decode("/fuzz/a.anim", head, body) {
        for c in clips.iter().take(8) {
            let _ = c.channels(false);
            let _ = c.channels(true);
            let _ = c.root_displacement();
        }
    }
});
